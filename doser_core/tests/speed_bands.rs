use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doser_core::{Calibration, ControlCfg, Doser, DosingStatus, FilterCfg, Timeouts};
use doser_traits::clock::Clock;
use rstest::rstest;

/// Deterministic clock: `sleep` advances a virtual offset instead of blocking.
///
/// Without this the dosing loop really sleeps one sampling period per sample,
/// which is what made this file take ~6 s of wall clock to assert a handful of
/// integers. Nothing here depends on real time, so nothing here should spend it.
#[derive(Clone)]
struct ManualClock {
    origin: Instant,
    offset: Arc<Mutex<Duration>>,
}
impl ManualClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            offset: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }
}
impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.origin + *self.offset.lock().unwrap()
    }
    fn sleep(&self, d: Duration) {
        *self.offset.lock().unwrap() += d;
    }
}

// Motor spy that records the last commanded speed
#[derive(Default, Clone)]
struct SpyMotor {
    pub last_sps: Arc<Mutex<u32>>,
}
impl doser_traits::Motor for SpyMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        *self.last_sps.lock().unwrap() = sps;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[rstest]
fn band_selection_matches_thresholds() {
    // Use default bands: [(1.0,1100),(0.5,450),(0.2,200)]
    // Use a calibration that provides 0.1 g/count resolution to avoid whole-gram rounding.
    // This ensures err_g matches the intended fractional values for band selection.
    //
    // Helper: step once at current_g and capture sps via spy
    let check = |current_g: f32, expect_sps: u32| {
        let spy = SpyMotor::default();
        let spy_ref = spy.clone();
        let mut d = Doser::builder()
            .with_scale(doser_core::mocks::NoopScale)
            .with_motor(spy)
            .with_filter(FilterCfg {
                ma_window: 1,
                median_window: 1,
                sample_rate_hz: 50,
                ema_alpha: 0.0,
            })
            .with_control(ControlCfg::default())
            .with_timeouts(Timeouts { sensor_ms: 1 })
            .with_calibration(Calibration {
                gain_g_per_count: 0.1,
                zero_counts: 0,
                offset_g: 0.0,
            })
            .with_target_grams(10.0)
            .with_clock(Box::new(ManualClock::new()))
            .apply_calibration::<()>(None)
            .build()
            .unwrap_or_else(|e| panic!("build doser: {e}"));
        // Convert grams to raw counts at 0.1 g/count resolution.
        let raw = (current_g * 10.0).round() as i32;
        let status = d
            .step_from_raw(raw)
            .unwrap_or_else(|e| panic!("step_from_raw({raw}): {e}"));
        assert!(
            matches!(status, DosingStatus::Running),
            "current_g={current_g}: expected Running, got {status:?}"
        );
        let sps = *spy_ref.last_sps.lock().unwrap();
        assert_eq!(sps, expect_sps, "current_g={current_g}");
    };

    check(8.8, 1100); // err_g=1.2
    check(9.3, 450); // err_g=0.7
    check(9.7, 200); // err_g=0.3
    check(9.9, 200); // err_g=0.1 -> lowest band (avoid rounding to 10.0)
}

// ── Simulated auger for the band-vs-legacy comparison ────────────────────────

#[derive(Default)]
struct SimState {
    weight_g: f32,
    sps: u32,
    running: bool,
}

/// Mass accumulates only while the motor is running, at the commanded rate.
/// Readings are raw counts at 0.01 g/count, i.e. centigrams — the previous
/// whole-gram resolution quantized every final weight to exactly 5.00 g, which
/// made the overshoot comparison below unable to observe anything.
struct SimScale {
    st: Arc<Mutex<SimState>>,
    h: f32,
}
impl doser_traits::Scale for SimScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        let mut st = self.st.lock().unwrap();
        if st.running {
            st.weight_g += (st.sps as f32) * self.h / 50.0; // 50Hz step emulation
        }
        Ok((st.weight_g * 100.0).round() as i32)
    }
}
struct SimMotor {
    st: Arc<Mutex<SimState>>,
}
impl doser_traits::Motor for SimMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.st.lock().unwrap().running = true;
        Ok(())
    }
    fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.st.lock().unwrap().sps = sps;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.st.lock().unwrap().running = false;
        Ok(())
    }
}

const TARGET_G: f32 = 5.0;
/// `ControlCfg::default().epsilon_g`; the completion zone opens this far below
/// the target, so a well-behaved run lands in `[target - epsilon, target]`.
const EPSILON_G: f32 = 0.08;

/// Build one simulated run; `speed_bands` empty selects the legacy taper.
fn sim_run(speed_bands: Vec<(f32, u32)>) -> Doser {
    let st = Arc::new(Mutex::new(SimState::default()));
    Doser::builder()
        .with_scale(SimScale {
            st: st.clone(),
            h: 0.002,
        })
        .with_motor(SimMotor { st })
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            speed_bands,
            stable_ms: 0,
            ..ControlCfg::default()
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_calibration(Calibration {
            gain_g_per_count: 0.01,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(TARGET_G)
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"))
}

/// Drive to completion, returning the number of samples consumed. Every step
/// result is inspected: the previous version stepped twice per iteration and
/// discarded the first `Result` with `let _ =`, so a hardware error or an abort
/// on that step was silently swallowed and the sim advanced twice as fast as
/// the loop believed.
fn drive_to_completion(doser: &mut Doser, label: &str) -> usize {
    for step in 1..=1_000 {
        match doser
            .step()
            .unwrap_or_else(|e| panic!("{label} step {step}: {e}"))
        {
            DosingStatus::Running => {}
            DosingStatus::Complete => return step,
            DosingStatus::Aborted(e) => panic!("{label} aborted at step {step}: {e}"),
        }
    }
    panic!("{label} did not complete within 1000 steps");
}

#[rstest]
fn banded_vs_legacy_overshoot() {
    let mut doser_legacy = sim_run(vec![]);
    doser_legacy.begin();
    let legacy_steps = drive_to_completion(&mut doser_legacy, "legacy");

    let mut doser_band = sim_run(ControlCfg::default().speed_bands);
    doser_band.begin();
    let band_steps = drive_to_completion(&mut doser_band, "banded");

    // Both must land inside the acceptance window rather than merely stopping.
    for (label, w) in [
        ("legacy", doser_legacy.last_weight()),
        ("banded", doser_band.last_weight()),
    ] {
        assert!(
            (TARGET_G - EPSILON_G..=TARGET_G).contains(&w),
            "{label} finished at {w} g, outside [{}, {TARGET_G}]",
            TARGET_G - EPSILON_G
        );
    }

    // The band table exists to reach the target at least as quickly as the
    // legacy proportional taper, which crawls through the final gram.
    assert!(
        band_steps <= legacy_steps,
        "banded run took {band_steps} samples vs legacy {legacy_steps}"
    );

    // Overshoot itself: kept as the original regression, but note it is weak by
    // construction here — `epsilon_g` opens the completion zone *below* the
    // target, so with this auger rate both runs finish at 4.92 g and both
    // overshoots are 0. The in-band and sample-count assertions above are what
    // actually discriminate; this one would only fire if a change let either
    // strategy sail past the target.
    let o1 = (doser_legacy.last_weight() - TARGET_G).max(0.0);
    let o2 = (doser_band.last_weight() - TARGET_G).max(0.0);
    assert!(o2 <= o1 + 1e-3, "banded overshoot={o2} legacy={o1}");
}

//! Regression tests for the hardening pass:
//! - the motor is actually stopped on every safety-abort path,
//! - persisted calibration `offset_g` survives the conversion pipeline,
//! - realistic small calibration gains are no longer quantized to zero,
//! - the settle window is bounded above: a reading over the acceptance band but
//!   inside `max_overshoot_g` completes instead of livelocking to `max_run_ms`.

use std::error::Error;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use doser_core::error::{AbortReason, DoserError};
use doser_core::{Calibration, ControlCfg, Doser, DosingStatus, FilterCfg, SafetyCfg, Timeouts};
use doser_traits::clock::Clock;
use doser_traits::{Motor, Scale};

/// Deterministic clock: `sleep` advances a virtual offset instead of blocking,
/// so time-based safety paths (max-run, no-progress, settle) are reproducible.
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

/// Motor that records whether `stop()` was called (observable after the run via
/// a cloned `Arc`, since the builder takes ownership of the motor).
#[derive(Clone, Default)]
struct RecordingMotor {
    stopped: Arc<AtomicBool>,
}
impl Motor for RecordingMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, _sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.stopped.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Scale returning a constant raw reading.
struct ConstScale(i32);
impl Scale for ConstScale {
    fn read(&mut self, _t: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        Ok(self.0)
    }
}

/// Scale returning a fixed sequence, then repeating the last value.
struct SeqScale {
    seq: Vec<i32>,
    idx: usize,
}
impl Scale for SeqScale {
    fn read(&mut self, _t: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        let v = self
            .seq
            .get(self.idx)
            .copied()
            .unwrap_or_else(|| self.seq.last().copied().unwrap_or(0));
        self.idx += 1;
        Ok(v)
    }
}

/// `gain = 1.0 g/count` makes raw counts equal grams, for readable test values.
fn unit_cal() -> Calibration {
    Calibration {
        gain_g_per_count: 1.0,
        zero_counts: 0,
        offset_g: 0.0,
    }
}

fn passthrough_filter(sample_rate_hz: u32) -> FilterCfg {
    FilterCfg {
        ma_window: 1,
        median_window: 1,
        sample_rate_hz,
        ema_alpha: 0.0,
    }
}

fn run_to_terminal(mut doser: Doser, max_steps: usize) -> DosingStatus {
    doser.begin();
    for _ in 0..max_steps {
        match doser.step().expect("step ok") {
            DosingStatus::Running => continue,
            other => return other,
        }
    }
    panic!("did not reach a terminal status within {max_steps} steps");
}

#[test]
fn motor_stops_on_overshoot() {
    let stopped = Arc::new(AtomicBool::new(false));
    let doser = Doser::builder()
        .with_scale(ConstScale(10)) // 10 g, well over target+overshoot
        .with_motor(RecordingMotor {
            stopped: stopped.clone(),
        })
        .with_filter(passthrough_filter(100))
        .with_control(ControlCfg {
            speed_bands: vec![],
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 60_000,
            max_overshoot_g: 1.0,
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        })
        .with_calibration(unit_cal())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(5.0)
        .with_clock(Box::new(ManualClock::new()))
        .build()
        .unwrap();

    let status = run_to_terminal(doser, 16);
    assert!(
        matches!(
            status,
            DosingStatus::Aborted(DoserError::Abort(AbortReason::Overshoot))
        ),
        "expected Overshoot abort"
    );
    assert!(
        stopped.load(Ordering::SeqCst),
        "motor must be stopped on overshoot"
    );
}

#[test]
fn motor_stops_on_estop() {
    let stopped = Arc::new(AtomicBool::new(false));
    let doser = Doser::builder()
        .with_scale(ConstScale(1))
        .with_motor(RecordingMotor {
            stopped: stopped.clone(),
        })
        .with_filter(passthrough_filter(100))
        .with_control(ControlCfg {
            speed_bands: vec![],
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg::default())
        .with_calibration(unit_cal())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(5.0)
        .with_estop_check(|| true)
        .with_estop_debounce(1)
        .with_clock(Box::new(ManualClock::new()))
        .build()
        .unwrap();

    let status = run_to_terminal(doser, 16);
    assert!(
        matches!(
            status,
            DosingStatus::Aborted(DoserError::Abort(AbortReason::Estop))
        ),
        "expected Estop abort"
    );
    assert!(
        stopped.load(Ordering::SeqCst),
        "motor must be stopped on E-stop"
    );
}

#[test]
fn motor_stops_on_max_runtime() {
    let stopped = Arc::new(AtomicBool::new(false));
    let doser = Doser::builder()
        .with_scale(ConstScale(1)) // never reaches the 5 g target
        .with_motor(RecordingMotor {
            stopped: stopped.clone(),
        })
        .with_filter(passthrough_filter(100)) // 10 ms period
        .with_control(ControlCfg {
            speed_bands: vec![],
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 50,
            max_overshoot_g: 2.0,
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        })
        .with_calibration(unit_cal())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(5.0)
        .with_clock(Box::new(ManualClock::new()))
        .build()
        .unwrap();

    let status = run_to_terminal(doser, 1000);
    assert!(
        matches!(
            status,
            DosingStatus::Aborted(DoserError::Abort(AbortReason::MaxRuntime))
        ),
        "expected MaxRuntime abort"
    );
    assert!(
        stopped.load(Ordering::SeqCst),
        "motor must be stopped on max-runtime"
    );
}

#[test]
fn motor_stops_on_no_progress() {
    let stopped = Arc::new(AtomicBool::new(false));
    let doser = Doser::builder()
        .with_scale(ConstScale(1)) // registers once, then never changes
        .with_motor(RecordingMotor {
            stopped: stopped.clone(),
        })
        .with_filter(passthrough_filter(100))
        .with_control(ControlCfg {
            speed_bands: vec![],
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 10_000,
            max_overshoot_g: 2.0,
            no_progress_epsilon_g: 0.05,
            no_progress_ms: 50,
        })
        .with_calibration(unit_cal())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(5.0)
        .with_clock(Box::new(ManualClock::new()))
        .build()
        .unwrap();

    let status = run_to_terminal(doser, 1000);
    assert!(
        matches!(
            status,
            DosingStatus::Aborted(DoserError::Abort(AbortReason::NoProgress))
        ),
        "expected NoProgress abort"
    );
    assert!(
        stopped.load(Ordering::SeqCst),
        "motor must be stopped on no-progress"
    );
}

#[test]
fn persisted_offset_g_survives_conversion() {
    use doser_config::{Calibration as CfgCal, PersistedCalibration};

    let pc = PersistedCalibration {
        gain_g_per_count: 0.01,
        zero_counts: 100,
        offset_g: 0.5,
    };
    // PersistedCalibration -> doser_config::Calibration keeps offset_g (previously dropped).
    let cfg_cal = CfgCal::from(pc);
    assert!((cfg_cal.offset_g - 0.5).abs() < 1e-6);
    // doser_config::Calibration -> doser_core::Calibration keeps offset_g.
    let core_cal = Calibration::from(&cfg_cal);
    assert!((core_cal.offset_g - 0.5).abs() < 1e-6);
    // ...and it is actually applied: at zero delta, to_cg == offset (0.5 g = 50 cg).
    assert_eq!(core_cal.to_cg(100), 50);
}

#[test]
fn small_gain_calibration_reads_correctly() {
    // README example: 100 g spans 182000 counts -> ~0.000549 g/count. The old
    // integer-cg/count quantization collapsed this to 0; the scaled fixed-point
    // must read ~100 g (10000 cg).
    let cal = Calibration {
        gain_g_per_count: 100.0 / 182_000.0,
        zero_counts: 0,
        offset_g: 0.0,
    };
    let cg = cal.to_cg(182_000);
    assert!((cg - 10_000).abs() <= 5, "expected ~10000 cg, got {cg}");

    // A tiny gain the old code would zero out still yields a nonzero reading.
    let tiny = Calibration {
        gain_g_per_count: 0.0001,
        zero_counts: 0,
        offset_g: 0.0,
    };
    assert!(tiny.to_cg(100_000) > 0);
}

/// Run a reading sequence and return the number of steps to complete.
///
/// Panics if the run does not complete within 1000 steps, which is what makes it
/// a livelock detector as well as a timing probe.
fn steps_to_complete(readings: Vec<i32>) -> usize {
    let mut doser = Doser::builder()
        .with_scale(SeqScale {
            seq: readings,
            idx: 0,
        })
        .with_motor(RecordingMotor::default())
        .with_filter(passthrough_filter(100)) // 10 ms period
        .with_control(ControlCfg {
            speed_bands: vec![],
            slow_at_g: 1.0,
            hysteresis_g: 0.2, // ±0.2 g acceptance band
            stable_ms: 30,     // 3 periods in-band required
            coarse_speed: 1200,
            fine_speed: 250,
            epsilon_g: 0.0,
        })
        .with_safety(SafetyCfg {
            max_run_ms: 100_000,
            max_overshoot_g: 5.0, // tolerate the spike without an overshoot abort
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        })
        .with_calibration(unit_cal())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .with_clock(Box::new(ManualClock::new()))
        .build()
        .unwrap();

    doser.begin();
    for step in 1..=1000 {
        match doser.step().expect("step ok") {
            DosingStatus::Running => {}
            DosingStatus::Complete => return step,
            DosingStatus::Aborted(e) => panic!("unexpected abort: {e}"),
        }
    }
    panic!("did not complete");
}

/// A reading *above* the acceptance band must not extend the settle window.
///
/// This test previously asserted the opposite — that a high out-of-band spike
/// restarts the settle timer. That rule is unbounded: the completion-zone branch
/// stops the motor and returns before the motor-command section, so nothing in
/// the loop can bring a high reading back down. The old test only passed because
/// its fixture made the spike transient; the same code path with a *persistent*
/// high reading (which is what beans still in flight when the motor stops
/// actually produce) restarted the timer on every subsequent sample forever, burned
/// the whole `max_run_ms`, and reported a finished, slightly over-delivered dose
/// as `MaxRuntime`. `persistent_over_delivery_completes_instead_of_livelocking`
/// below is the case the old rule could not survive.
///
/// The low-side delay behaviour is unaffected and is still covered: a dip out of
/// the completion zone clears the settle timer and restarts the motor
/// (`doser_core/tests/doser.rs::a_dip_out_of_the_completion_zone_resets_the_settle_timer`
/// and `doser_core/tests/motor_restart.rs`).
#[test]
fn a_high_out_of_band_spike_does_not_extend_the_settle_window() {
    // Baseline: stays in band -> settles after `stable_ms`.
    let baseline = steps_to_complete(vec![9, 10]);
    // Same approach with one high out-of-band spike (13 g, |err| = 3 g > the
    // 0.2 g band, still inside the 5 g overshoot cap) mid-settle.
    let with_spike = steps_to_complete(vec![9, 10, 10, 13, 10]);
    assert_eq!(
        with_spike, baseline,
        "a high spike must not delay settle (baseline={baseline}, with_spike={with_spike})"
    );
}

/// The property that actually hardens the loop: a dose that settles persistently
/// above the acceptance band, but within `max_overshoot_g`, must complete in
/// bounded time rather than spinning until `max_run_ms`.
///
/// `steps_to_complete` panics after 1000 steps, so a livelock fails loudly here.
#[test]
fn persistent_over_delivery_completes_instead_of_livelocking() {
    // Target 10 g, band 0.2 g, overshoot cap 5 g. The weight settles at 13 g:
    // 2.8 g outside the band and 2 g inside the cap, and it never comes back
    // down (`SeqScale` repeats its last value forever).
    let baseline = steps_to_complete(vec![9, 10]);
    let over = steps_to_complete(vec![9, 13]);
    assert_eq!(
        over, baseline,
        "an over-delivered dose inside the overshoot cap must settle on the same \
         schedule as an in-band one (baseline={baseline}, over={over})"
    );
}

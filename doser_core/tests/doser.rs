use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, SafetyCfg, Timeouts};
use doser_traits::clock::Clock;
use doser_traits::{Motor, Scale};
use rstest::rstest;

/// Deterministic clock: `sleep` advances a virtual offset instead of blocking.
/// Tests that only care about sample *sequence* should use this so they do not
/// spend one real sampling period per step.
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

/// Scale that returns a fixed sequence, then repeats the last value.
struct SeqScale {
    seq: Vec<i32>,
    idx: usize,
}
impl SeqScale {
    fn new(seq: impl Into<Vec<i32>>) -> Self {
        Self {
            seq: seq.into(),
            idx: 0,
        }
    }
}
impl Scale for SeqScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        let v = if self.idx < self.seq.len() {
            let x = self.seq[self.idx];
            self.idx += 1;
            x
        } else {
            self.seq.last().copied().unwrap_or(0)
        };
        Ok(v)
    }
}

/// Motor spy (minimal)
#[derive(Default)]
struct SpyMotor {
    stopped: bool,
}
impl Motor for SpyMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, _sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.stopped = true;
        Ok(())
    }
}

#[rstest]
fn ema_converges_on_step_input() {
    // Step input: 0 for a few samples, then constant 100 (centigrams => 1g).
    // EMA with alpha=0.5 should converge near 100 within tolerance after enough steps.
    struct StepScale {
        step_at: usize,
        idx: usize,
    }
    impl StepScale {
        fn new(step_at: usize) -> Self {
            Self { step_at, idx: 0 }
        }
    }
    impl Scale for StepScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            self.idx += 1;
            if self.idx >= self.step_at {
                Ok(100)
            } else {
                Ok(0)
            }
        }
    }

    let mut doser = Doser::builder()
        .with_scale(StepScale::new(5))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
            ema_alpha: 0.5,
        })
        .with_control(ControlCfg {
            speed_bands: vec![],
            stable_ms: 0,
            ..ControlCfg::default()
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap();

    // Run for sufficient steps post-step to converge near 100
    for _ in 0..50 {
        let _ = doser.step().unwrap();
    }
    let y = doser.last_weight();
    // within 0.1g of the step level (1.0g) is acceptable for alpha=0.5 over 50 samples
    assert!((y - 1.0).abs() <= 0.1, "EMA did not converge: y={y}");
}

#[rstest]
fn completes_when_in_band_and_settled() {
    // Target exactly present in the sequence -> completes immediately when hit.
    let scale = SeqScale::new([10, 15, 17, 18]);
    let motor = SpyMotor { stopped: false };

    let control = ControlCfg {
        speed_bands: vec![],
        slow_at_g: 1.0,
        hysteresis_g: 0.1, // ±0.1 g band
        stable_ms: 0,      // complete immediately when in-band
        coarse_speed: 1200,
        fine_speed: 250,
        epsilon_g: 0.0,
    };

    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(motor)
        .with_filter(FilterCfg::default())
        .with_control(control)
        // Interpret raw counts as grams for this test
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 1.0,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_target_grams(18.0) // exact hit in sequence
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    for _ in 0..100 {
        match doser.step().unwrap_or_else(|e| panic!("step ok: {e}")) {
            DosingStatus::Running => continue,
            DosingStatus::Complete => return, // success
            DosingStatus::Aborted(e) => panic!("aborted: {e}"),
        }
    }
    panic!("did not complete within 100 steps");
}

#[rstest]
fn propagates_scale_error_as_core_error() {
    struct ErrScale;
    impl Scale for ErrScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            Err("boom".into())
        }
    }

    let mut doser = Doser::builder()
        .with_scale(ErrScale)
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    let err = doser
        .step()
        .expect_err("step should error on scale failure");

    // eyre::Report carries a typed source; verify it maps to our domain error
    match err
        .downcast_ref::<doser_core::error::DoserError>()
        .expect("expected DoserError inside Report")
    {
        doser_core::error::DoserError::Hardware(s) => assert!(s.contains("boom")),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[rstest]
fn stops_immediately_when_target_crossed() {
    // Sequence crosses target (overshoot). Should Complete immediately when w >= target if stable_ms == 0.
    let scale = SeqScale::new([5, 9, 10, 11]);
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            speed_bands: vec![],
            stable_ms: 0,
            ..ControlCfg::default()
        })
        // Interpret raw counts as grams
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 1.0,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    // Steps until we cross 10g.
    assert!(matches!(
        doser.step().unwrap_or_else(|e| panic!("step: {e}")),
        DosingStatus::Running
    )); // 5
    assert!(matches!(
        doser.step().unwrap_or_else(|e| panic!("step: {e}")),
        DosingStatus::Running
    )); // 9
    // At 10, inside hysteresis and stable_ms==0 => Complete
    assert!(matches!(
        doser.step().unwrap_or_else(|e| panic!("step: {e}")),
        DosingStatus::Complete
    )); // 10
}

#[rstest]
fn aborts_on_excessive_overshoot() {
    // Configure small overshoot threshold to trigger abort when we jump past target.
    let safety = SafetyCfg {
        max_run_ms: 60_000,
        max_overshoot_g: 0.5,
        no_progress_epsilon_g: 0.0,
        no_progress_ms: 0,
    };
    let scale = SeqScale::new([8, 9, 11]); // target 10, overshoot by 1g > 0.5
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_safety(safety)
        // Interpret raw counts as grams
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 1.0,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    // 8 -> running
    assert!(matches!(
        doser.step().unwrap_or_else(|e| panic!("step: {e}")),
        DosingStatus::Running
    ));
    // 9 -> running
    assert!(matches!(
        doser.step().unwrap_or_else(|e| panic!("step: {e}")),
        DosingStatus::Running
    ));
    // 11 -> abort due to overshoot guard
    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("overshoot")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[rstest]
fn aborts_on_max_runtime() {
    // Use 0ms runtime to trigger immediately after begin().
    let safety = SafetyCfg {
        max_run_ms: 0,
        max_overshoot_g: 10.0,
        no_progress_epsilon_g: 0.0,
        no_progress_ms: 0,
    };
    let mut doser = Doser::builder()
        .with_scale(SeqScale::new([0]))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_safety(safety)
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(1.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    doser.begin();
    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("max run time")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[rstest]
fn calibration_converts_counts_to_grams() {
    // gain 0.5 g/count, zero at 0, offset 0 => raw 10 -> 5g
    let scale = SeqScale::new([10]);
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 0.5,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(100.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Running | DosingStatus::Complete | DosingStatus::Aborted(_) => {}
    }
    assert!((doser.last_weight() - 5.0).abs() < 1e-6);
}

#[rstest]
fn tare_zero_counts_shifts_baseline() {
    // zero_counts=100, gain 1 => raw 100 -> 0g; raw 105 -> 5g
    let mut doser = Doser::builder()
        .with_scale(SeqScale::new([100, 105]))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 1.0,
            zero_counts: 100,
            offset_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(1000.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    let _ = doser.step();
    assert!((doser.last_weight() - 0.0).abs() < 1e-6);
    let _ = doser.step();
    assert!((doser.last_weight() - 5.0).abs() < 1e-6);
}

#[rstest]
fn median_filter_suppresses_spike() {
    // Sequence with a spike at the third reading; median_window=3 should suppress it.
    struct SeqScale {
        seq: Vec<i32>,
        idx: usize,
    }
    impl SeqScale {
        fn new(seq: impl Into<Vec<i32>>) -> Self {
            Self {
                seq: seq.into(),
                idx: 0,
            }
        }
    }
    impl Scale for SeqScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            let v = if self.idx < self.seq.len() {
                let x = self.seq[self.idx];
                self.idx += 1;
                x
            } else {
                *self.seq.last().unwrap()
            };
            Ok(v)
        }
    }

    let mut doser = Doser::builder()
        .with_scale(SeqScale::new([0, 0, 1000, 0, 0]))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 3,
            sample_rate_hz: 50,
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            speed_bands: vec![],
            slow_at_g: 1000.0,
            hysteresis_g: 0.01,
            stable_ms: 0,
            coarse_speed: 1,
            fine_speed: 1,
            epsilon_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(1000.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    // Step through first two zeros
    let _ = doser.step().unwrap_or_else(|e| panic!("step: {e}"));
    let _ = doser.step().unwrap_or_else(|e| panic!("step: {e}"));
    // Third reading is 1000, but median of [0,0,1000] = 0 => last_weight should remain ~0
    let _ = doser.step().unwrap_or_else(|e| panic!("step: {e}"));
    assert!(
        doser.last_weight().abs() < 1e-3,
        "median filter did not suppress spike: {}",
        doser.last_weight()
    );
}

#[rstest]
fn requires_time_to_settle_when_stable_ms_positive() {
    // When stable_ms > 0, entering the hysteresis band should not complete immediately.
    let scale = SeqScale::new([9, 10, 10, 10]);
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            speed_bands: vec![],
            slow_at_g: 1.0,
            hysteresis_g: 1.0,
            stable_ms: 10_000,
            coarse_speed: 1200,
            fine_speed: 250,
            epsilon_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    // First read 9 -> Running
    assert!(matches!(
        doser.step().unwrap_or_else(|e| panic!("step: {e}")),
        DosingStatus::Running
    ));
    // Now in-band at 10, but stable_ms is large, so should still be Running (not Complete)
    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Running => {}
        other => panic!("expected Running before stable_ms elapsed, got {other:?}"),
    }
}

#[rstest]
fn aborts_on_no_progress_watchdog() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    struct ConstScale(i32);
    impl Scale for ConstScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            Ok(self.0)
        }
    }
    // Deterministic test clock
    #[derive(Clone)]
    struct TestClock {
        origin: std::time::Instant,
        ms: Arc<AtomicU64>,
    }
    impl TestClock {
        fn new() -> Self {
            Self {
                origin: std::time::Instant::now(),
                ms: Arc::new(AtomicU64::new(0)),
            }
        }
        fn advance(&self, ms: u64) {
            self.ms.fetch_add(ms, Ordering::Relaxed);
        }
    }
    impl doser_traits::clock::Clock for TestClock {
        fn now(&self) -> std::time::Instant {
            self.origin + std::time::Duration::from_millis(self.ms.load(Ordering::Relaxed))
        }
        fn sleep(&self, d: std::time::Duration) {
            let add = d.as_millis() as u64;
            if add > 0 {
                self.advance(add);
            }
        }
    }

    let safety = SafetyCfg {
        max_run_ms: 60_000,
        max_overshoot_g: 10.0,
        no_progress_epsilon_g: 0.01,
        no_progress_ms: 5,
    };

    let tclk = TestClock::new();

    let mut doser = Doser::builder()
        .with_scale(ConstScale(0))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            speed_bands: vec![],
            slow_at_g: 1.0,
            hysteresis_g: 100.0,
            stable_ms: 10_000,
            coarse_speed: 1200,
            fine_speed: 250,
            epsilon_g: 0.0,
        })
        .with_safety(safety)
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_clock(Box::new(tclk.clone()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    // First step should run
    assert!(matches!(
        doser.step().unwrap_or_else(|e| panic!("step: {e}")),
        DosingStatus::Running
    ));
    // Advance virtual time to exceed the watchdog window
    tclk.advance(10);
    // Next step should hit watchdog and abort (no progress)
    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("no progress")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[rstest]
fn aborts_on_no_progress_when_below_epsilon_for_window() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    struct ConstScale(i32);
    impl Scale for ConstScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            Ok(self.0)
        }
    }
    #[derive(Clone)]
    struct TestClock {
        origin: std::time::Instant,
        ms: Arc<AtomicU64>,
    }
    impl TestClock {
        fn new() -> Self {
            Self {
                origin: std::time::Instant::now(),
                ms: Arc::new(AtomicU64::new(0)),
            }
        }
        fn advance(&self, ms: u64) {
            self.ms.fetch_add(ms, Ordering::Relaxed);
        }
    }
    impl doser_traits::clock::Clock for TestClock {
        fn now(&self) -> std::time::Instant {
            self.origin + std::time::Duration::from_millis(self.ms.load(Ordering::Relaxed))
        }
        fn sleep(&self, d: std::time::Duration) {
            let add = d.as_millis() as u64;
            if add > 0 {
                self.advance(add);
            }
        }
    }

    let safety = SafetyCfg {
        max_run_ms: 60_000,
        max_overshoot_g: 10.0,
        no_progress_epsilon_g: 0.02,
        no_progress_ms: 25,
    };
    let tclk = TestClock::new();
    let mut doser = Doser::builder()
        .with_scale(ConstScale(0))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_safety(safety)
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_clock(Box::new(tclk.clone()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap();

    // First step starts running and initializes progress tracker
    assert!(matches!(doser.step().unwrap(), DosingStatus::Running));
    // Hover within epsilon repeatedly and advance time beyond window
    for _ in 0..5 {
        let _ = doser.step().unwrap();
        tclk.advance(5);
    }
    match doser.step().unwrap() {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("no progress")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[rstest]
fn estop_condition_latches_until_begin() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct ConstScale(i32);
    impl Scale for ConstScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            Ok(self.0)
        }
    }

    let estop = Arc::new(AtomicBool::new(true));
    let estop_clone = estop.clone();

    let mut doser = Doser::builder()
        .with_scale(ConstScale(0))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            speed_bands: vec![],
            slow_at_g: 1.0,
            hysteresis_g: 100.0,
            stable_ms: 10_000,
            coarse_speed: 1200,
            fine_speed: 250,
            epsilon_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_estop_debounce(1)
        .with_estop_check(move || estop_clone.load(Ordering::Relaxed))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));

    // First step sees estop=true -> Aborted
    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("estop")),
        other => panic!("expected Aborted(estop), got {other:?}"),
    }

    // Clear estop, but latch should keep aborting until begin() resets it
    estop.store(false, Ordering::Relaxed);
    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("estop")),
        other => panic!("expected latched Aborted(estop), got {other:?}"),
    }

    // Reset run; latch cleared in begin(); should now run
    doser.begin();
    match doser.step().unwrap_or_else(|e| panic!("step: {e}")) {
        DosingStatus::Running | DosingStatus::Aborted(_) | DosingStatus::Complete => {}
    }
}

/// `epsilon_g` exists so the completion zone is entered *before* the target is
/// crossed. Without it, a run whose sample-to-sample step straddles the target
/// jumps from "still below the zone" straight past the overshoot limit and
/// aborts. This pins that behaviour in both directions.
///
/// The scale/motor pair below is a local, fully scripted simulation rather than
/// `doser_hardware::sim_pair()` driven by `DOSER_TEST_SIM_INC`: that env var is
/// process-global, so setting it races every other test in the binary, and the
/// increment it produced could not actually trip the overshoot guard (with a
/// 0.12 g step and a 0.05 g limit, the first reading at or past 5.00 g is
/// 5.04 g, which lands *inside* the completion zone rather than past the
/// 5.05 g abort line — so the assertion was permanently unreachable).
#[rstest]
fn overshoot_epsilon_regression() {
    /// Shared auger state: mass only accumulates while the motor is running.
    #[derive(Default)]
    struct SimState {
        cg: i32,
        running: bool,
    }

    /// Advances by exactly `inc_cg` per read while running; 1 count == 1 cg
    /// (calibration gain 0.01 g/count), so every value below is exact.
    struct SimScale {
        st: Arc<Mutex<SimState>>,
        inc_cg: i32,
    }
    impl Scale for SimScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            let mut st = self.st.lock().unwrap();
            if st.running {
                st.cg += self.inc_cg;
            }
            Ok(st.cg)
        }
    }
    struct SimMotor {
        st: Arc<Mutex<SimState>>,
        stops: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Motor for SimMotor {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.st.lock().unwrap().running = true;
            Ok(())
        }
        fn set_speed(&mut self, _sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.st.lock().unwrap().running = false;
            self.stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    // 0.12 g per sample against a 5.00 g target with a 0.03 g overshoot limit:
    //   epsilon 0.00 -> zone starts at 500 cg; samples are 0,12,...,492,504.
    //                   492 is below the zone, 504 > 503 -> Overshoot abort.
    //   epsilon 0.08 -> zone starts at 492 cg; the 492 sample completes the run
    //                   and stops the motor, so 504 is never produced.
    const INC_CG: i32 = 12;
    let safety = SafetyCfg {
        max_run_ms: 5_000,
        max_overshoot_g: 0.03,
        no_progress_epsilon_g: 0.0,
        no_progress_ms: 0,
    };

    let build = |epsilon_g: f32| {
        let st = Arc::new(Mutex::new(SimState::default()));
        let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let doser = Doser::builder()
            .with_scale(SimScale {
                st: st.clone(),
                inc_cg: INC_CG,
            })
            .with_motor(SimMotor {
                st: st.clone(),
                stops: stops.clone(),
            })
            .with_filter(FilterCfg {
                ma_window: 1,
                median_window: 1,
                sample_rate_hz: 50,
                ema_alpha: 0.0,
            })
            .with_control(ControlCfg {
                epsilon_g,
                stable_ms: 0,
                ..ControlCfg::default()
            })
            .with_safety(safety.clone())
            .with_timeouts(Timeouts { sensor_ms: 10 })
            .with_calibration(doser_core::Calibration {
                gain_g_per_count: 0.01,
                zero_counts: 0,
                offset_g: 0.0,
            })
            .with_target_grams(5.0)
            .with_clock(Box::new(ManualClock::new()))
            .apply_calibration::<()>(None)
            .build()
            .unwrap_or_else(|e| panic!("build doser: {e}"));
        (doser, stops)
    };

    let run = |doser: &mut Doser| -> DosingStatus {
        for k in 1..=200 {
            match doser.step().unwrap_or_else(|e| panic!("step {k}: {e}")) {
                DosingStatus::Running => {}
                other => return other,
            }
        }
        panic!("run did not terminate within 200 steps");
    };

    // epsilon 0.0 -> the step straddles the completion zone and trips the guard.
    let (mut doser_zero, stops_zero) = build(0.0);
    doser_zero.begin();
    match run(&mut doser_zero) {
        DosingStatus::Aborted(doser_core::error::DoserError::Abort(reason)) => assert_eq!(
            reason,
            doser_core::error::AbortReason::Overshoot,
            "expected an overshoot abort with epsilon_g=0.0"
        ),
        other => panic!("expected Aborted(Overshoot) with epsilon_g=0.0, got {other:?}"),
    }
    assert!(
        (doser_zero.last_weight() - 5.04).abs() < 1e-6,
        "overshoot abort should fire on the 5.04 g sample, got {}",
        doser_zero.last_weight()
    );
    assert!(
        stops_zero.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the overshoot abort must stop the motor"
    );

    // epsilon 0.08 -> the same increment completes one sample earlier instead.
    let (mut doser_eps, stops_eps) = build(0.08);
    doser_eps.begin();
    match run(&mut doser_eps) {
        DosingStatus::Complete => {}
        other => panic!("expected Complete with epsilon_g=0.08, got {other:?}"),
    }
    assert!(
        (doser_eps.last_weight() - 4.92).abs() < 1e-6,
        "expected completion at 4.92 g, got {}",
        doser_eps.last_weight()
    );
    assert!(
        stops_eps.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "completion must stop the motor"
    );
}

/// A dose that settles *above* the acceptance band but within `max_overshoot_g`
/// must complete, not spin until `max_run_ms`.
///
/// This was a livelock. The settle timer was restarted by any out-of-band
/// reading, including a high one. But the completion-zone branch stops the motor
/// and returns before the motor-command section, so once the weight is above the
/// band nothing in the loop can bring it back down: the timer restarted on every
/// subsequent sample forever, and the run burned the whole `max_run_ms` before
/// aborting `MaxRuntime` — misreporting a finished, slightly over-delivered dose
/// as a stalled one. Beans still in flight when the motor stops make this the
/// ordinary case on real hardware, not a corner case.
///
/// Fixture: target 5.00 g, band = max(hysteresis 0.07, epsilon 0.08) = 8 cg, so
/// the band tops out at 5.08 g. The scale settles at 5.10 g — 2 cg above the
/// band and well inside the 0.60 g overshoot cap.
#[rstest]
fn settling_above_the_band_but_within_overshoot_completes() {
    let mut doser = Doser::builder()
        .with_scale(SeqScale::new([400, 490, 510]))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50, // 20 ms period on the manual clock
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            stable_ms: 100,
            epsilon_g: 0.08,
            hysteresis_g: 0.07,
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 60_000,
            max_overshoot_g: 0.6,
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 0.01,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(5.0)
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));
    doser.begin();

    // 100 ms of settle at a 20 ms period is 5 samples; 64 is generous headroom
    // and still far short of the 60 s cap, so a hang shows up as MaxRuntime.
    let mut status = DosingStatus::Running;
    for k in 1..=64 {
        status = doser.step().unwrap_or_else(|e| panic!("step {k}: {e}"));
        if !matches!(status, DosingStatus::Running) {
            break;
        }
    }
    assert!(
        matches!(status, DosingStatus::Complete),
        "a dose settled 0.02 g above the band and 0.58 g inside the overshoot cap \
         must complete, got {status:?}"
    );
    assert!(
        (doser.last_weight() - 5.10).abs() < 1e-6,
        "final weight {}",
        doser.last_weight()
    );
}

/// The companion to the test above: the settle timer must still be reset by a
/// reading that leaves the completion zone downward, so the fix above is not a
/// blanket "never reset the timer" that would complete a short dose on stale
/// elapsed time.
///
/// This is the *reachable* form of the recovery path. An in-zone reading can
/// never be below the acceptance band — the zone opens at `target - epsilon`
/// and the band opens at `target - max(hysteresis, epsilon)`, at or below it —
/// so a dip deep enough to matter has already left the zone and is handled by
/// the `else` arm, which clears the timer and restarts the motor.
#[rstest]
fn a_dip_out_of_the_completion_zone_resets_the_settle_timer() {
    // Target 5.00 g, epsilon 0.08 g, so the zone opens at 4.92 g. `stable_ms` is
    // 60 ms at a 20 ms period, so completion needs the zone held for 3 samples.
    //
    // Sequence: 4.00 (below), 4.95 (entry at t=20 ms), four samples at 4.80 g
    // (out of the zone), then 4.95 g again (re-entry at t=120 ms). The dip is
    // deliberately long enough that 100 ms have passed since the *original*
    // entry: if the timer were not cleared, the very first sample back in the
    // zone would already read >= 60 ms elapsed and complete on stale time. With
    // the clear, the timer restarts at re-entry and three more samples are
    // needed.
    let mut doser = Doser::builder()
        .with_scale(SeqScale::new([
            400, 495, 480, 480, 480, 480, 495, 495, 495, 495,
        ]))
        .with_motor(SpyMotor { stopped: false })
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50, // 20 ms period
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            stable_ms: 60,
            epsilon_g: 0.08,
            hysteresis_g: 0.07,
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 60_000,
            max_overshoot_g: 0.6,
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 0.01,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(5.0)
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));
    doser.begin();

    // Steps 1-9 must all be Running. Step 7 is the load-bearing one: it is the
    // re-entry sample, and it completes here only if the dip failed to clear the
    // timer. Steps 8-9 are the restarted timer not yet having elapsed.
    for k in 1..=9 {
        let status = doser.step().unwrap_or_else(|e| panic!("step {k}: {e}"));
        assert!(
            matches!(status, DosingStatus::Running),
            "step {k} should still be Running (the dip must reset the settle \
             timer), got {status:?}"
        );
    }

    // Step 10 carries the restarted timer to exactly `stable_ms`.
    let status = doser.step().unwrap_or_else(|e| panic!("step 10: {e}"));
    assert!(
        matches!(status, DosingStatus::Complete),
        "the dose should complete once the restarted settle timer elapses, got {status:?}"
    );
}

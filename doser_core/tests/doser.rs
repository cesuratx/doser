use std::error::Error;
use std::time::Duration;

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, SafetyCfg, Timeouts};
use doser_traits::{Motor, Scale};

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

#[test]
fn completes_when_in_band_and_settled() {
    // Target exactly present in the sequence -> completes immediately when hit.
    let scale = SeqScale::new([10, 15, 17, 18]);
    let motor = SpyMotor::default();

    let control = ControlCfg {
        slow_at_g: 1.0,
        hysteresis_g: 0.1, // ±0.1 g band
        stable_ms: 0,      // complete immediately when in-band
        coarse_speed: 1200,
        fine_speed: 250,
    };

    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(motor)
        .with_filter(FilterCfg::default())
        .with_control(control)
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_target_grams(18.0) // exact hit in sequence
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    for _ in 0..100 {
        match doser.step().expect("step ok") {
            DosingStatus::Running => continue,
            DosingStatus::Complete => return, // success
            DosingStatus::Aborted(e) => panic!("aborted: {e}"),
        }
    }
    panic!("did not complete within 100 steps");
}

#[test]
fn propagates_scale_error_as_core_error() {
    struct ErrScale;
    impl Scale for ErrScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            Err("boom".into())
        }
    }

    let mut doser = Doser::builder()
        .with_scale(ErrScale)
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    let err = doser
        .step()
        .expect_err("step should error on scale failure");
    let msg = format!("{err}");
    assert!(msg.contains("hardware"), "unexpected error: {msg}");
}

#[test]
fn stops_immediately_when_target_crossed() {
    // Sequence crosses target (overshoot). Should Complete immediately when w >= target if stable_ms == 0.
    let scale = SeqScale::new([5, 9, 10, 11]);
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            stable_ms: 0,
            ..ControlCfg::default()
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    // Steps until we cross 10g.
    assert!(matches!(doser.step().unwrap(), DosingStatus::Running)); // 5
    assert!(matches!(doser.step().unwrap(), DosingStatus::Running)); // 9
    // At 10, inside hysteresis and stable_ms==0 => Complete
    assert!(matches!(doser.step().unwrap(), DosingStatus::Complete)); // 10
}

#[test]
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
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_safety(safety)
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    // 8 -> running
    assert!(matches!(doser.step().unwrap(), DosingStatus::Running));
    // 9 -> running
    assert!(matches!(doser.step().unwrap(), DosingStatus::Running));
    // 11 -> abort due to overshoot guard
    match doser.step().unwrap() {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("overshoot")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[test]
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
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_safety(safety)
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(1.0)
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    doser.begin();
    match doser.step().unwrap() {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("max run time")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[test]
fn calibration_converts_counts_to_grams() {
    // gain 0.5 g/count, zero at 0, offset 0 => raw 10 -> 5g
    let scale = SeqScale::new([10]);
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(SpyMotor::default())
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
        .expect("build doser");

    match doser.step().unwrap() {
        DosingStatus::Running | DosingStatus::Complete | DosingStatus::Aborted(_) => {}
    }
    assert!((doser.last_weight() - 5.0).abs() < 1e-6);
}

#[test]
fn tare_zero_counts_shifts_baseline() {
    // zero_counts=100, gain 1 => raw 100 -> 0g; raw 105 -> 5g
    let mut doser = Doser::builder()
        .with_scale(SeqScale::new([100, 105]))
        .with_motor(SpyMotor::default())
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
        .expect("build doser");

    let _ = doser.step();
    assert!((doser.last_weight() - 0.0).abs() < 1e-6);
    let _ = doser.step();
    assert!((doser.last_weight() - 5.0).abs() < 1e-6);
}

#[test]
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
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 3,
            sample_rate_hz: 50,
        })
        .with_control(ControlCfg {
            slow_at_g: 1000.0,
            hysteresis_g: 0.01,
            stable_ms: 0,
            coarse_speed: 1,
            fine_speed: 1,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(1000.0)
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    // Step through first two zeros
    let _ = doser.step().unwrap();
    let _ = doser.step().unwrap();
    // Third reading is 1000, but median of [0,0,1000] = 0 => last_weight should remain ~0
    let _ = doser.step().unwrap();
    assert!(
        doser.last_weight().abs() < 1e-3,
        "median filter did not suppress spike: {}",
        doser.last_weight()
    );
}

#[test]
fn requires_time_to_settle_when_stable_ms_positive() {
    // When stable_ms > 0, entering the hysteresis band should not complete immediately.
    let scale = SeqScale::new([9, 10, 10, 10]);
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            slow_at_g: 1.0,
            hysteresis_g: 1.0,
            stable_ms: 10_000,
            coarse_speed: 1200,
            fine_speed: 250,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    // First read 9 -> Running
    assert!(matches!(doser.step().unwrap(), DosingStatus::Running));
    // Now in-band at 10, but stable_ms is large, so should still be Running (not Complete)
    match doser.step().unwrap() {
        DosingStatus::Running => {}
        other => panic!("expected Running before stable_ms elapsed, got {other:?}"),
    }
}

#[test]
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
    struct TestClock(Arc<AtomicU64>);
    impl TestClock {
        fn new() -> Self {
            Self(Arc::new(AtomicU64::new(0)))
        }
        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::Relaxed);
        }
    }
    impl doser_core::Clock for TestClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
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
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            slow_at_g: 1.0,
            hysteresis_g: 100.0,
            stable_ms: 10_000,
            coarse_speed: 1200,
            fine_speed: 250,
        })
        .with_safety(safety)
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_clock(tclk.clone())
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    // First step should run
    assert!(matches!(doser.step().unwrap(), DosingStatus::Running));
    // Advance virtual time to exceed the watchdog window
    tclk.advance(10);
    // Next step should hit watchdog and abort (no progress)
    match doser.step().unwrap() {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("no progress")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[test]
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
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            slow_at_g: 1.0,
            hysteresis_g: 100.0,
            stable_ms: 10_000,
            coarse_speed: 1200,
            fine_speed: 250,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_estop_check(move || estop_clone.load(Ordering::Relaxed))
        .apply_calibration::<()>(None)
        .build()
        .expect("build doser");

    // First step sees estop=true -> Aborted
    match doser.step().unwrap() {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("estop")),
        other => panic!("expected Aborted(estop), got {other:?}"),
    }

    // Clear estop, but latch should keep aborting until begin() resets it
    estop.store(false, Ordering::Relaxed);
    match doser.step().unwrap() {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("estop")),
        other => panic!("expected latched Aborted(estop), got {other:?}"),
    }

    // Reset run; latch cleared in begin(); should now run
    doser.begin();
    match doser.step().unwrap() {
        DosingStatus::Running | DosingStatus::Aborted(_) | DosingStatus::Complete => {}
    }
}

use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};
use rstest::rstest;

/// Scale that returns a constant below-target value
struct ConstScale(i32);
impl Scale for ConstScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        Ok(self.0)
    }
}

#[derive(Default)]
struct SpyMotor;
impl Motor for SpyMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, _sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[rstest]
fn aborts_immediately_on_estop() {
    // E-stop flag toggled externally simulates GPIO read.
    let estop = Arc::new(AtomicBool::new(false));
    let estop_ref = estop.clone();

    let mut doser = Doser::builder()
        .with_scale(ConstScale(0))
        .with_motor(SpyMotor)
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_estop_debounce(1)
        .with_estop_check(move || estop_ref.load(Ordering::Relaxed))
        .build()
        .expect("doser build");

    // First step: no estop -> Running
    match doser.step().unwrap_or_else(|e| panic!("step 1: {e}")) {
        DosingStatus::Running => {}
        other => panic!("expected Running, got {other:?}"),
    }

    // Trip estop and expect Aborted at next step
    estop.store(true, Ordering::Relaxed);
    match doser.step().unwrap_or_else(|e| panic!("step 2: {e}")) {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("estop")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

#[rstest]
fn estop_bounce_does_not_trip_with_debounce_n3() {
    let pattern = [true, false, true, false, true];
    let idx = Arc::new(AtomicUsize::new(0));
    let idx_ref = idx.clone();

    let mut doser = Doser::builder()
        .with_scale(ConstScale(0))
        .with_motor(SpyMotor)
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_estop_debounce(3)
        .with_estop_check(move || {
            let i = idx_ref.fetch_add(1, Ordering::Relaxed);
            let i = i.min(pattern.len() - 1);
            pattern[i]
        })
        .build()
        .expect("doser build");

    for _ in 0..pattern.len() {
        match doser.step().unwrap() {
            DosingStatus::Running => {}
            DosingStatus::Aborted(e) => panic!("should not trip: {e}"),
            _ => {}
        }
    }
}

#[rstest]
fn estop_long_press_trips_and_latches_until_begin() {
    let mut doser = Doser::builder()
        .with_scale(ConstScale(0))
        .with_motor(SpyMotor)
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_estop_debounce(3)
        .with_estop_check(|| true) // always pressed
        .build()
        .expect("doser build");

    // Need three consecutive pressed polls to latch
    match doser.step().unwrap() {
        DosingStatus::Running => {}
        other => panic!("expected Running, got {other:?}"),
    }
    match doser.step().unwrap() {
        DosingStatus::Running => {}
        other => panic!("expected Running, got {other:?}"),
    }
    match doser.step().unwrap() {
        DosingStatus::Aborted(_) => {}
        other => panic!("expected Aborted, got {other:?}"),
    }

    // Subsequent step remains aborted
    match doser.step().unwrap() {
        DosingStatus::Aborted(_) => {}
        other => panic!("expected Aborted, got {other:?}"),
    }

    // Reset run; latch cleared in begin(); requires debounce again
    doser.begin();
    match doser.step().unwrap() {
        DosingStatus::Running => {}
        other => panic!("expected Running, got {other:?}"),
    }
    match doser.step().unwrap() {
        DosingStatus::Running => {}
        other => panic!("expected Running, got {other:?}"),
    }
    match doser.step().unwrap() {
        DosingStatus::Aborted(_) => {}
        other => panic!("expected Aborted, got {other:?}"),
    }
}

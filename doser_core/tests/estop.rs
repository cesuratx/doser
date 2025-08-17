use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};

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

#[test]
fn aborts_immediately_on_estop() {
    // E-stop flag toggled externally simulates GPIO read.
    let estop = Arc::new(AtomicBool::new(false));
    let estop_ref = estop.clone();

    let mut doser = Doser::builder()
        .with_scale(ConstScale(0))
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_target_grams(10.0)
        .with_estop_check(move || estop_ref.load(Ordering::Relaxed))
        .build()
        .expect("doser build");

    // First step: no estop -> Running
    match doser.step().unwrap() {
        DosingStatus::Running => {}
        other => panic!("expected Running, got {other:?}"),
    }

    // Trip estop and expect Aborted at next step
    estop.store(true, Ordering::Relaxed);
    match doser.step().unwrap() {
        DosingStatus::Aborted(e) => assert!(format!("{e}").contains("estop")),
        other => panic!("expected Aborted, got {other:?}"),
    }
}

use doser_core::error::DoserError;
use doser_core::{ControlCfg, Doser, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};
use std::error::Error;
use std::time::Duration;

#[derive(Default)]
struct DummyScale;
impl Scale for DummyScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        Ok(0)
    }
}

#[derive(Default)]
struct DummyMotor;
impl Motor for DummyMotor {
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
fn builder_requires_scale_motor_target() {
    // Missing everything
    let err = match Doser::builder().build() {
        Err(e) => e,
        Ok(_) => panic!("should fail without fields"),
    };
    assert_is_config_err(err);

    // Missing motor
    let err = match Doser::builder()
        .with_scale(DummyScale::default())
        .with_target_grams(10.0)
        .build()
    {
        Err(e) => e,
        Ok(_) => panic!("should fail when motor is missing"),
    };
    assert_is_config_err(err);

    // Missing scale
    let err = match Doser::builder()
        .with_motor(DummyMotor::default())
        .with_target_grams(10.0)
        .build()
    {
        Err(e) => e,
        Ok(_) => panic!("should fail when scale is missing"),
    };
    assert_is_config_err(err);

    // Missing target
    let err = match Doser::builder()
        .with_scale(DummyScale::default())
        .with_motor(DummyMotor::default())
        .build()
    {
        Err(e) => e,
        Ok(_) => panic!("should fail when target is missing"),
    };
    assert_is_config_err(err);
}

#[test]
fn builder_accepts_defaults() {
    let res = Doser::builder()
        .with_scale(DummyScale::default())
        .with_motor(DummyMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts::default())
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build();

    match res {
        Ok(_) => {} // success
        Err(e) => panic!("builder with defaults should succeed, got error: {e}"),
    }
}

fn assert_is_config_err(err: DoserError) {
    match err {
        DoserError::Config(_) => {}
        other => panic!("expected Config error, got: {other:?}"),
    }
}

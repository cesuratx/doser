use doser_core::error::BuildError;
use doser_core::{ControlCfg, Doser, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};
use rstest::rstest;
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

#[rstest]
fn builder_validates_target_range() {
    // Too small
    let err = match Doser::builder()
        .with_scale(DummyScale::default())
        .with_motor(DummyMotor::default())
        .with_target_grams(0.0)
        .build()
    {
        Err(e) => e,
        Ok(_) => panic!("should fail for target out of range"),
    };
    assert_is_config_err(err);

    // Too large
    let err = match Doser::builder()
        .with_scale(DummyScale::default())
        .with_motor(DummyMotor::default())
        .with_target_grams(10_000.0)
        .build()
    {
        Err(e) => e,
        Ok(_) => panic!("should fail for target out of range"),
    };
    assert_is_config_err(err);
}

#[rstest]
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

#[rstest]
fn builder_missing_scale_yields_build_error() {
    // Intentionally omit with_scale()
    let err = Doser::builder()
        .with_motor(DummyMotor::default())
        .with_target_grams(10.0)
        .try_build()
        .expect_err("expected build error due to missing scale");

    match err.downcast_ref::<BuildError>() {
        Some(BuildError::MissingScale) => {}
        other => panic!("expected MissingScale, got: {other:?}"),
    }
}

fn assert_is_config_err(err: doser_core::error::Report) {
    match err.downcast_ref::<BuildError>() {
        Some(BuildError::InvalidConfig(_)) => {}
        other => panic!("expected InvalidConfig build error, got: {other:?}"),
    }
}

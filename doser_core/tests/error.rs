use std::error::Error;
use std::time::Duration;

use doser_core::error::DoserError;
use doser_core::{ControlCfg, Doser, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};
use rstest::rstest;

/// A scale that returns OK once, then errors — to exercise error at a non-first step.
struct FlakyScaleTimeout {
    ok_sent: bool,
}
impl Scale for FlakyScaleTimeout {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        if self.ok_sent {
            Err("sensor timeout".into())
        } else {
            self.ok_sent = true;
            Ok(0)
        }
    }
}

/// A scale that returns OK once, then a non-timeout hardware error.
struct FlakyScaleOtherErr {
    ok_sent: bool,
}
impl Scale for FlakyScaleOtherErr {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        if self.ok_sent {
            Err("sensor disconnected".into())
        } else {
            self.ok_sent = true;
            Ok(0)
        }
    }
}

#[derive(Default)]
struct NopMotor;
impl Motor for NopMotor {
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
fn timeouts_map_to_dosererror_timeout() {
    let mut doser = Doser::builder()
        .with_scale(FlakyScaleTimeout { ok_sent: false })
        .with_motor(NopMotor)
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            slow_at_g: 1.0,
            hysteresis_g: 0.1,
            stable_ms: 0,
            coarse_speed: 1000,
            fine_speed: 200,
            epsilon_g: 0.0,
        })
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_target_grams(0.5)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build: {e}"));

    // First step OK, second should error:
    let _ = doser.step().unwrap_or_else(|e| panic!("step1: {e}"));
    let err = doser.step().expect_err("expected timeout error");
    let de = err
        .downcast_ref::<DoserError>()
        .expect("expected typed DoserError in eyre::Report");
    match de {
        DoserError::Timeout => {}
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[rstest]
fn non_timeout_hardware_errors_map_to_dosererror_hardware() {
    let mut doser = Doser::builder()
        .with_scale(FlakyScaleOtherErr { ok_sent: false })
        .with_motor(NopMotor)
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_target_grams(0.5)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build: {e}"));

    let _ = doser.step().unwrap_or_else(|e| panic!("step1: {e}"));
    let err = doser.step().expect_err("expected hardware error");
    let de = err
        .downcast_ref::<DoserError>()
        .expect("expected typed DoserError in eyre::Report");
    match de {
        DoserError::Hardware(_) => {}
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[rstest]
fn typed_hw_timeout_maps_to_timeout() {
    #[derive(Default)]
    struct NopMotor;
    impl Motor for NopMotor {
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

    struct FlakyTypedTimeout {
        ok_sent: bool,
    }
    impl Scale for FlakyTypedTimeout {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            if self.ok_sent {
                Err(Box::new(doser_hardware::error::HwError::Timeout))
            } else {
                self.ok_sent = true;
                Ok(0)
            }
        }
    }

    let mut doser = Doser::builder()
        .with_scale(FlakyTypedTimeout { ok_sent: false })
        .with_motor(NopMotor)
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_target_grams(0.5)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build: {e}"));

    let _ = doser.step().unwrap_or_else(|e| panic!("step1: {e}"));
    let err = doser.step().expect_err("expected timeout");
    let de = err
        .downcast_ref::<DoserError>()
        .expect("expected typed DoserError in eyre::Report");
    match de {
        DoserError::Timeout => {}
        other => panic!("unexpected: {other:?}"),
    }
}

#[rstest]
fn typed_hw_other_maps_to_hardware_fault() {
    #[derive(Default)]
    struct NopMotor;
    impl Motor for NopMotor {
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

    struct FlakyTypedOther {
        ok_sent: bool,
    }
    impl Scale for FlakyTypedOther {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            if self.ok_sent {
                Err(Box::new(doser_hardware::error::HwError::Gpio(
                    "boom".into(),
                )))
            } else {
                self.ok_sent = true;
                Ok(0)
            }
        }
    }

    let mut doser = Doser::builder()
        .with_scale(FlakyTypedOther { ok_sent: false })
        .with_motor(NopMotor)
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_target_grams(0.5)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build: {e}"));

    let _ = doser.step().unwrap_or_else(|e| panic!("step1: {e}"));
    let err = doser.step().expect_err("expected hardware fault");
    let de = err
        .downcast_ref::<DoserError>()
        .expect("expected typed DoserError in eyre::Report");
    match de {
        DoserError::HardwareFault(msg) => assert!(msg.contains("boom")),
        other => panic!("unexpected: {other:?}"),
    }
}

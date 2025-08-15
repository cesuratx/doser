use std::error::Error;
use std::time::Duration;

use doser_core::error::DoserError;
use doser_core::{ControlCfg, Doser, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};

/// A scale that returns OK once, then errors — to exercise error at a non-first step.
struct FlakyScale {
    ok_sent: bool,
}
impl Scale for FlakyScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        if self.ok_sent {
            Err("sensor timeout".into())
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

#[test]
fn hardware_errors_map_to_dosererror_hardware() {
    let mut doser = Doser::builder()
        .with_scale(FlakyScale { ok_sent: false })
        .with_motor(NopMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg {
            slow_at_g: 1.0,
            hysteresis_g: 0.1,
            stable_ms: 0,
            coarse_speed: 1000,
            fine_speed: 200,
        })
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_target_grams(0.5)
        .apply_calibration::<()>(None)
        .build()
        .unwrap();

    // First step OK, second should error:
    let _ = doser.step().unwrap();
    let err = doser.step().expect_err("expected hardware error");
    match err {
        DoserError::Hardware(_) => {}
        other => panic!("unexpected error variant: {other:?}"),
    }
}

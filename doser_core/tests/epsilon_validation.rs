use doser_core::{ControlCfg, Doser, FilterCfg, SafetyCfg, Timeouts};
use doser_traits::{Motor, Scale};
use std::error::Error;
use std::time::Duration;

#[derive(Default)]
struct NopScale;
impl Scale for NopScale {
    fn read(&mut self, _t: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> { Ok(0) }
}
#[derive(Default)]
struct NopMotor;
impl Motor for NopMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> { Ok(()) }
    fn set_speed(&mut self, _: u32) -> Result<(), Box<dyn Error + Send + Sync>> { Ok(()) }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> { Ok(()) }
}

#[test]
fn builder_rejects_negative_no_progress_epsilon() {
    let err = Doser::builder()
        .with_scale(NopScale::default())
        .with_motor(NopMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_safety(SafetyCfg { no_progress_epsilon_g: -0.1, ..Default::default() })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(5.0)
        .try_build()
        .expect_err("expected invalid config");
    let s = format!("{}", err);
    assert!(s.contains("no_progress_epsilon_g must be >= 0"));
}

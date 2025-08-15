use std::error::Error;
use std::time::Duration;

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, Timeouts};
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
            *self.seq.last().unwrap_or(&0)
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

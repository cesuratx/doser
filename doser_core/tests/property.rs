use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, SafetyCfg, Timeouts};
use proptest::prelude::*;

// A scale that returns a bounded monotonic sequence of centigrams when running.
#[derive(Default, Clone)]
struct BoundedScale {
    grams: f32,
    // deltas to apply per read while motor is running (g)
    deltas: Vec<f32>,
    idx: usize,
}

impl BoundedScale {
    fn new(deltas: Vec<f32>) -> Self {
        Self {
            grams: 0.0,
            deltas,
            idx: 0,
        }
    }
}

impl doser_traits::Scale for BoundedScale {
    fn read(
        &mut self,
        _timeout: std::time::Duration,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        // simulate that weight increases while motor is running according to prepared deltas
        let dg = if self.idx < self.deltas.len() {
            self.deltas[self.idx]
        } else {
            0.0
        };
        self.idx = self.idx.saturating_add(1);
        self.grams = (self.grams + dg).max(0.0);
        Ok((self.grams * 100.0) as i32)
    }
}

#[derive(Default)]
struct NoopMotor;
impl doser_traits::Motor for NoopMotor {
    fn set_speed(
        &mut self,
        _steps_per_sec: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

prop_compose! {
    fn deltas_strategy()(
        len in 20usize..200,
        max_delta in 1u32..20u32,
        stall_at in 5usize..50,
    ) -> Vec<f32> {
        // bounded per-step increase in grams (0 .. max_delta/100) until stall_at, then zeros
        let step = max_delta as f32 / 100.0;
        let mut v = Vec::with_capacity(len);
        for i in 0..len {
            if i < stall_at { v.push(step); } else { v.push(0.0); }
        }
        v
    }
}

proptest! {
    #[test]
    fn never_exceeds_max_overshoot_and_no_progress_triggers(deltas in deltas_strategy(), target in 1u32..50u32) {
        let scale = BoundedScale::new(deltas);
        let motor = NoopMotor::default();

        let filter = FilterCfg { ma_window: 1, median_window: 1, sample_rate_hz: 500, ema_alpha: 0.0 };
        let control = ControlCfg { stable_ms: 0, ..ControlCfg::default() };
        let safety = SafetyCfg {
            max_run_ms: 5_000,
            max_overshoot_g: 0.10, // 0.10 g overshoot cap
            no_progress_epsilon_g: 0.005,
            no_progress_ms: 10,
        };
        let timeouts = Timeouts { sensor_ms: 10 };

        let mut doser = Doser::builder()
            .with_scale(scale)
            .with_motor(motor)
            .with_filter(filter)
            .with_control(control)
            .with_safety(safety.clone())
            .with_timeouts(timeouts)
            .apply_calibration::<()>(None)
            .with_target_grams(target as f32)
            .build()
            .unwrap();
        doser.begin();

        let mut aborted_no_progress = false;
        let mut done = false;
        for _ in 0..5000 { // bounded steps
            match doser.step().unwrap() {
                DosingStatus::Running => continue,
                DosingStatus::Complete => { done = true; break; },
                DosingStatus::Aborted(e) => {
                    match e {
                        doser_core::error::DoserError::Abort(doser_core::error::AbortReason::NoProgress) => {
                            aborted_no_progress = true; break;
                        }
                        doser_core::error::DoserError::Abort(doser_core::error::AbortReason::Overshoot) => {
                            // Invariant: never exceed configured max_overshoot_g
                            let final_g = doser.last_weight();
                            let allowed = (target as f32) + safety.max_overshoot_g + 1e-6;
                            prop_assert!(final_g <= allowed, "overshoot {} > allowed {}", final_g, allowed);
                            break;
                        }
                        _ => break,
                    }
                }
            }
        }
        // If progress stalls (e.g., deltas become zero), NoProgress should be possible
        // We accept either completion or NoProgress depending on generated data
        prop_assert!(done || aborted_no_progress);
    }
}

use crate::Doser;
use crate::error::{DoserError, Result};

pub trait DosingStrategy {
    fn dose(&self, doser: &mut Doser) -> Result<crate::DosingResult>;
}

pub struct DefaultDosingStrategy;

impl DosingStrategy for DefaultDosingStrategy {
    fn dose(&self, doser: &mut Doser) -> Result<crate::DosingResult> {
        let mut attempts = 0;
        let max_attempts = 100;
        loop {
            attempts += 1;
            let status = doser.step()?;
            let avg_weight = doser.filtered_weight();
            let diff = (doser.target_grams - avg_weight).abs();
            // Adaptive motor control: slow down as we approach target
            if diff < 0.5 {
                doser.motor.stop();
            } else {
                doser.motor.start();
            }
            match status {
                crate::DosingStatus::Complete => {
                    doser.motor.stop();
                    return Ok(crate::DosingResult {
                        final_weight: avg_weight,
                        attempts,
                        error: None,
                    });
                }
                crate::DosingStatus::Aborted(e) => {
                    doser.motor.stop();
                    return Ok(crate::DosingResult {
                        final_weight: avg_weight,
                        attempts,
                        error: Some(*e),
                    });
                }
                crate::DosingStatus::Running => {}
            }
            if attempts >= max_attempts {
                doser.motor.stop();
                return Ok(crate::DosingResult {
                    final_weight: avg_weight,
                    attempts,
                    error: Some(DoserError::Config("Max attempts exceeded".to_string())),
                });
            }
        }
    }
}

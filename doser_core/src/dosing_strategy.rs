use crate::{Doser, DosingStatus, error::AbortReason, error::DoserError, error::Result};

pub struct DefaultDosingStrategy {
    pub max_attempts: u32,
}

impl Default for DefaultDosingStrategy {
    fn default() -> Self {
        Self { max_attempts: 100 }
    }
}

impl DefaultDosingStrategy {
    pub fn dose(&self, doser: &mut Doser) -> Result<()> {
        doser.begin();
        let mut attempts = 0_u32;
        loop {
            attempts += 1;
            match doser.step()? {
                DosingStatus::Complete => return Ok(()),
                DosingStatus::Running => { /* keep going */ }
                DosingStatus::Aborted(e) => {
                    let _ = doser.motor_stop();
                    return Err(eyre::eyre!(e.to_string()));
                }
            }
            if attempts >= self.max_attempts {
                let _ = doser.motor_stop();
                return Err(DoserError::Abort(AbortReason::MaxAttempts));
            }
        }
    }
}

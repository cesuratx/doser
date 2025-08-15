use crate::{
    Doser, DosingStatus,
    error::{DoserError, Result},
};

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
        let mut attempts = 0_u32;
        loop {
            attempts += 1;
            match doser.step()? {
                DosingStatus::Complete => return Ok(()),
                DosingStatus::Running => { /* keep going */ }
                DosingStatus::Aborted(e) => {
                    let _ = doser.motor_stop();
                    return Err(e);
                }
            }
            if attempts >= self.max_attempts {
                let _ = doser.motor_stop();
                return Err(DoserError::State("max attempts exceeded".into()));
            }
        }
    }
}

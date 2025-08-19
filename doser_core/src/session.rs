use crate::error::BuildError;
use crate::error::{DoserError, Result};

pub struct DosingSessionBuilder {
    grams: Option<f32>,
    // ... other fields
}

#[derive(Debug)]
pub struct DosingSession {
    grams: f32,
    // ...
}

impl DosingSessionBuilder {
    pub fn new() -> Self {
        Self {
            grams: None, /* ... */
        }
    }

    pub fn grams(mut self, g: f32) -> Self {
        self.grams = Some(g);
        self
    }

    pub fn build(self) -> Result<DosingSession> {
        let grams = self
            .grams
            .ok_or_else(|| eyre::Report::new(BuildError::MissingTarget))?;
        if !(0.1..=5000.0).contains(&grams) {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "grams out of range",
            )));
        }
        Ok(DosingSession { grams })
    }
}

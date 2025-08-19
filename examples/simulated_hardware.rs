//! Example: Simulated Hardware Implementation

use doser_core::hardware::{Motor, Scale};

pub struct SimScale;

impl Scale for SimScale {
    fn read_weight(&mut self) -> Result<f32, eyre::Report> {
        Ok(42.0) // Simulated value
    }
}

pub struct SimMotor;

impl Motor for SimMotor {
    fn start(&mut self) -> Result<(), eyre::Report> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), eyre::Report> {
        Ok(())
    }
}

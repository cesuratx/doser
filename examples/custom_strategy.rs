//! Example: Custom Dosing Strategy

use doser_core::dosing_strategy::{DosingContext, DosingStrategy};

pub struct MyStrategy;

impl DosingStrategy for MyStrategy {
    fn dose(&mut self, ctx: &mut DosingContext) -> Result<(), eyre::Report> {
        // Custom dosing logic here
        Ok(())
    }
}

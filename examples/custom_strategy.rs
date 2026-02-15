//! Example: Custom dosing loop using the builder API.
//!
//! Shows how to construct a `Doser` and drive the control loop manually,
//! enabling custom logic between iterations (logging, dynamic speed changes, etc.).

use doser_core::{Doser, DosingStatus};
use doser_hardware::{SimulatedMotor, SimulatedScale};

fn main() -> eyre::Result<()> {
    let mut doser = Doser::builder()
        .with_scale(SimulatedScale::new())
        .with_motor(SimulatedMotor::default())
        .with_target_grams(5.0)
        .build()?;

    doser.begin();
    let max_attempts = 10_000_u32;
    for attempt in 1..=max_attempts {
        match doser.step()? {
            DosingStatus::Complete => {
                println!(
                    "Done after {attempt} iterations: {:.2} g",
                    doser.last_weight()
                );
                return Ok(());
            }
            DosingStatus::Running => { /* custom logic here */ }
            DosingStatus::Aborted(e) => {
                let _ = doser.motor_stop();
                return Err(eyre::eyre!("aborted: {e}"));
            }
        }
    }
    let _ = doser.motor_stop();
    Err(eyre::eyre!("exceeded {max_attempts} iterations"))
}

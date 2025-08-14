//! Quick Start Example
//!
//! This example demonstrates how to set up and run a simulated dosing session using the Doser library.
//!
//! # Steps
//!
//! 1. Create a `DoserBuilder` to configure the dosing session.
//! 2. Attach simulated scale and motor implementations.
//! 3. Set the target dose in grams.
//! 4. Build the dosing session and run it.
//!
//! # Output
//!
//! On success, the dosing session will complete using simulated hardware.
//!
//! # Errors
//!
//! Any configuration or runtime errors will be returned as an `eyre::Report`.

use doser_core::builder::DoserBuilder;
use doser_core::hardware::{SimMotor, SimScale};

/// Runs a simulated dosing session with a target of 18.5 grams.
///
/// # Parameters
///
/// - No parameters; configuration is hardcoded for demonstration.
///
/// # Usage
///
/// This example is intended to be run as a standalone binary or via `cargo run --example quick_start`.
/// It demonstrates the minimal setup required to use the Doser library in simulation mode.
///
/// # Related Examples
///
/// - [`custom_strategy.rs`](custom_strategy.rs): Shows how to implement a custom dosing strategy.
/// - [`simulated_hardware.rs`](simulated_hardware.rs): Shows how to implement simulated hardware traits.
///
/// # Errors
///
/// Returns an error if configuration or dosing fails, surfaced as an `eyre::Report`.
///
/// # See Also
///
/// - [Doser README](../../README.md)
/// - [API Documentation](https://docs.rs/doser_core)
fn main() -> Result<(), eyre::Report> {
    // Step 1: Create a new dosing session builder
    let mut builder = DoserBuilder::new();

    // Step 2: Attach simulated scale and motor
    // SimScale implements the `Scale` trait for simulation purposes.
    // SimMotor implements the `Motor` trait for simulation purposes.
    builder.scale(Box::new(SimScale));
    builder.motor(Box::new(SimMotor));

    // Step 3: Set target dose in grams
    builder.grams(18.5);

    // Step 4: Build and run the dosing session
    // Returns an error if configuration is invalid or dosing fails.
    let mut doser = builder.build()?;

    // Example: Real-time step-wise dosing loop (100Hz)
    use std::time::{Duration, Instant};
    let interval = Duration::from_millis(10); // 100Hz

    // Mock status enum for demonstration
    #[derive(PartialEq)]
    enum DosingStatus {
        Running,
        Complete,
        Error,
    }

    // Real-time, precise weight measurement with moving average filter
    const WINDOW_SIZE: usize = 10;
    let mut weight_window = [0.0f32; WINDOW_SIZE];
    let mut idx = 0;
    let mut cycles = 0;
    let max_cycles = 100;
    let mut status = DosingStatus::Running;
    while status == DosingStatus::Running {
        let start = Instant::now();
        // Simulate reading weight from scale
        let weight = SimScale.read_weight().unwrap_or(0.0);
        weight_window[idx % WINDOW_SIZE] = weight;
        idx += 1;
        // Calculate moving average
        let avg_weight: f32 = weight_window.iter().sum::<f32>() / WINDOW_SIZE as f32;
        println!(
            "Cycle {}: raw = {:.2}, avg = {:.2}",
            cycles, weight, avg_weight
        );

        // Simulate dosing logic (replace with doser.step() in real code)
        cycles += 1;
        if cycles >= max_cycles {
            status = DosingStatus::Complete;
        }
        let elapsed = start.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
    }
    // Optionally, handle completion or error
    if status == DosingStatus::Complete {
        println!("Dosing complete.");
    } else if status == DosingStatus::Error {
        println!("Dosing error.");
    }
    Ok(())
}

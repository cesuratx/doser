//! Quick Start Example
//!
//! This example demonstrates how to set up and run a simulated dosing session using the Doser library.

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, Timeouts};
use doser_hardware::{SimulatedMotor as SimMotor, SimulatedScale as SimScale};
use doser_traits::MonotonicClock;
use std::time::Duration;

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
    // Local monotonic clock for timing in this example
    let clock = MonotonicClock::new();

    // Build a Doser with simulated hardware and pass a clock into the builder
    let mut doser = Doser::builder()
        .with_scale(SimScale::default())
        .with_motor(SimMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 10 })
        // Simulated scale returns counts ≈ grams * 1000; convert to grams
        .with_calibration_gain_offset(0.001, 0.0)
        .with_target_grams(18.5)
        .with_clock(Box::new(clock.clone()))
        .build()?;

    // Optional: start a new run
    doser.begin();

    // 50 ms tick
    let tick = Duration::from_millis(50);
    // Throttle prints to ~200 ms
    let mut last_print = clock.now();

    loop {
        match doser.step()? {
            DosingStatus::Running => {
                // Throttled print of the last observed weight
                if clock.ms_since(last_print) >= 200 {
                    println!("weight = {:.3} g", doser.last_weight());
                    last_print = clock.now();
                }
            }
            DosingStatus::Complete => {
                println!("Dosing complete at {:.3} g", doser.last_weight());
                break;
            }
            DosingStatus::Aborted(e) => {
                println!("Dosing aborted: {e}");
                break;
            }
        }
        clock.sleep(tick);
    }

    Ok(())
}

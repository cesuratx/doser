//! Quick start: run a full dosing session against the shipped simulation backend.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p doser_cli --example quick_start
//! ```
//!
//! This is the shortest path from "nothing" to "a completed dose": build a
//! `Doser` with the builder, call `begin()`, then pump `step()` until it reports
//! `Complete`. No Raspberry Pi required — `doser_hardware::sim::sim_pair()`
//! returns a linked simulated scale/motor whose reading advances while the
//! motor runs.
//!
//! Related examples:
//! - `simulated_hardware` — implement the `Scale`/`Motor` traits yourself.
//! - `custom_strategy` — own the sampling loop and feed raw counts to the core.

/// The simulated run. Gated on the same cfg as `doser_hardware::sim`: a real Pi
/// build (`--features hardware` on Linux) compiles the simulation backend out
/// entirely, so the stub `main` at the bottom takes over there. The imports sit
/// inside the function so they are gated along with it.
#[cfg(any(not(feature = "hardware"), not(target_os = "linux")))]
fn main() -> Result<(), eyre::Report> {
    use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, Timeouts};
    use doser_hardware::sim::sim_pair;
    use doser_traits::{Clock, MonotonicClock};

    /// Grams the simulated scale gains per read while the motor is running.
    /// The simulator is driven by this env var (see README); default it so the
    /// example runs with zero setup, but let an explicit value win.
    const SIM_INC: &str = "DOSER_TEST_SIM_INC";

    if std::env::var_os(SIM_INC).is_none() {
        // SAFETY: no other thread exists yet — this is the first statement of
        // `main`, before the Doser (or anything else) spawns a sampler thread.
        unsafe { std::env::set_var(SIM_INC, "0.25") };
    }

    // Local monotonic clock, used here only to throttle the progress printout.
    // `MonotonicClock` is `Copy`, so the Doser gets its own handle on the same
    // timebase and this one stays usable below.
    let clock = MonotonicClock::new();

    // A linked pair: the scale's reading responds to the motor running.
    let (scale, motor) = sim_pair();
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(motor)
        .with_filter(FilterCfg {
            // 200 Hz keeps the example brisk; the control loop sleeps one
            // period between steps, so this also sets the tick rate.
            sample_rate_hz: 200,
            ..FilterCfg::default()
        })
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 10 })
        // The simulated scale reports raw counts at 0.01 g per count.
        .with_calibration_gain_offset(0.01, 0.0)
        .with_target_grams(18.5)
        .with_clock(Box::new(clock))
        .build()?;

    // Reset per-run state (settle timer, filters, watchdogs) before dosing.
    doser.begin();

    let mut last_print = clock.now();
    loop {
        match doser.step()? {
            DosingStatus::Running => {
                // `step()` already paced itself by one sample period, so the
                // loop just throttles printing to ~100 ms.
                if clock.ms_since(last_print) >= 100 {
                    println!("weight = {:.2} g", doser.last_weight());
                    last_print = clock.now();
                }
            }
            DosingStatus::Complete => {
                println!("dosing complete at {:.2} g", doser.last_weight());
                break;
            }
            DosingStatus::Aborted(e) => {
                // Safety aborts (overshoot, no progress, max runtime, e-stop)
                // arrive here as a value, not as an `Err`.
                println!("dosing aborted: {e}");
                break;
            }
        }
    }

    // The core stops the motor on both the completion and abort paths; this is
    // the belt-and-braces call you would also make from a Ctrl-C handler.
    doser.motor_stop()?;
    Ok(())
}

/// Real-hardware builds have no simulation backend to drive.
#[cfg(all(feature = "hardware", target_os = "linux"))]
fn main() {
    eprintln!(
        "quick_start drives the simulation backend, which is compiled out of a \
         --features hardware build. Re-run without that feature, or use \
         `doser_cli --config etc/doser_config.toml health` on real hardware."
    );
}

//! Implementing the `Scale` and `Motor` traits by hand.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p doser_cli --example simulated_hardware
//! ```
//!
//! `doser_core` never talks to a GPIO pin; it talks to `doser_traits::Scale`
//! and `doser_traits::Motor`. Anything implementing those two traits can be
//! dosed with — the shipped `doser_hardware::sim_pair()`, the real HX711 +
//! stepper backend, or a bench emulator like the one below.
//!
//! Two things the trait contract insists on, and that this example shows:
//!
//! 1. **`Scale::read` returns raw ADC counts, not grams.** Converting counts to
//!    grams is `doser_core`'s job, via the `Calibration` you hand the builder.
//! 2. **Errors are boxed and `Send + Sync`.** A driver reports a sensor timeout
//!    as `Err`; the core maps it onto its own abort taxonomy.
//!
//! Related examples:
//! - `quick_start` — the shipped simulation backend, minimal setup.
//! - `custom_strategy` — own the sampling loop and feed raw counts to the core.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};

/// ADC counts per gram. Real HX711 rigs land somewhere near this after
/// calibration; the reciprocal is the `gain_g_per_count` handed to the builder.
const COUNTS_PER_GRAM: f32 = 100.0;

/// Grams delivered per motor step. Times the commanded steps-per-second this
/// gives the flow rate the emulated hopper sees.
const GRAMS_PER_STEP: f32 = 0.000_2;

/// State shared between the emulated motor and the emulated scale, so the
/// reading responds to what the control loop commands. Real hardware couples
/// these through a pile of coffee beans; here it is two atomics.
#[derive(Debug, Default)]
struct Auger {
    running: AtomicBool,
    sps: AtomicU32,
}

/// Emulated load cell + ADC. Returns raw counts, with a ±1 count dither so the
/// filter configuration below has something to actually do.
struct BenchScale {
    auger: Arc<Auger>,
    grams: f32,
    /// xorshift state for the dither
    rng: u32,
}

impl BenchScale {
    fn new(auger: Arc<Auger>) -> Self {
        Self {
            auger,
            grams: 0.0,
            rng: 0x2545_F491,
        }
    }

    /// One count of dither in {-1, 0, +1}.
    fn dither(&mut self) -> i32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x % 3) as i32 - 1
    }
}

impl Scale for BenchScale {
    fn read(&mut self, timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        // An HX711 at 10 SPS needs ~100 ms per conversion. Reject a timeout that
        // could never be satisfied instead of blocking — this is the error path
        // the core turns into a sensor-timeout abort.
        if timeout < Duration::from_millis(2) {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "conversion not ready within timeout",
            )));
        }

        if self.auger.running.load(Ordering::Acquire) {
            let sps = self.auger.sps.load(Ordering::Acquire) as f32;
            self.grams += sps * GRAMS_PER_STEP;
        }

        Ok((self.grams * COUNTS_PER_GRAM) as i32 + self.dither())
    }
}

/// Emulated stepper driver. Prints the speed schedule the controller chooses,
/// which is the clearest way to see the coarse → fine speed bands in action.
struct BenchMotor {
    auger: Arc<Auger>,
}

impl BenchMotor {
    fn new(auger: Arc<Auger>) -> Self {
        Self { auger }
    }
}

impl Motor for BenchMotor {
    fn set_speed(&mut self, steps_per_sec: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        // `set_speed` is called every control step; only announce real changes.
        if steps_per_sec != self.auger.sps.swap(steps_per_sec, Ordering::Release) {
            println!("  motor: speed -> {steps_per_sec} sps");
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.auger.running.store(true, Ordering::Release);
        println!("  motor: start");
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.auger.sps.store(0, Ordering::Release);
        self.auger.running.store(false, Ordering::Release);
        Ok(())
    }

    // `set_direction` has a no-op default; only backends with a DIR line
    // (the real `HardwareMotor`) need to override it.
}

fn main() -> Result<(), eyre::Report> {
    // One shared rig, two handles — exactly how `sim_pair()` links its pair.
    let auger = Arc::new(Auger::default());
    let scale = BenchScale::new(Arc::clone(&auger));
    let motor = BenchMotor::new(Arc::clone(&auger));

    let target_g = 5.0;
    let mut doser = Doser::builder()
        .with_scale(scale)
        .with_motor(motor)
        .with_filter(FilterCfg {
            // Median first to reject the dither spikes, then a short moving
            // average to smooth what is left.
            median_window: 3,
            ma_window: 4,
            sample_rate_hz: 200,
            ..FilterCfg::default()
        })
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 20 })
        // Inverse of COUNTS_PER_GRAM: this is what turns counts into grams.
        .with_calibration_gain_offset(1.0 / COUNTS_PER_GRAM, 0.0)
        // Tare: the reading the empty cup produces. Zero for this emulator, but
        // on real hardware this is the value you capture before dosing.
        .with_tare_counts(0)
        .with_target_grams(target_g)
        .build()?;

    println!("dosing {target_g:.2} g with a hand-written scale/motor pair");
    doser.begin();

    let mut steps = 0_u32;
    loop {
        steps += 1;
        match doser.step()? {
            DosingStatus::Running => {}
            DosingStatus::Complete => {
                println!(
                    "complete: {:.2} g after {steps} control steps",
                    doser.last_weight()
                );
                break;
            }
            DosingStatus::Aborted(e) => {
                println!("aborted after {steps} control steps: {e}");
                break;
            }
        }
    }

    doser.motor_stop()?;
    Ok(())
}

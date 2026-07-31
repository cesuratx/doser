//! Owning the sampling loop: a custom strategy on top of `doser_core`.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p doser_cli --example custom_strategy
//! ```
//!
//! `Doser::step()` reads the scale for you. That is the easy path, but it ties
//! the control cadence to the sensor's blocking read. When you want to own the
//! sampling — oversample the ADC, decimate a fast sensor, splice in a second
//! sensor, or drive the loop from a DRDY interrupt — use `step_from_raw(counts)`
//! instead: you supply the raw ADC value and `doser_core` still owns everything
//! that matters (calibration, filtering, speed bands, settle logic, safety
//! watchdogs). This is the same split the CLI's `SamplingMode::Paced` uses.
//!
//! The strategy demonstrated here:
//! - 4x oversampling averaged in the outer loop before each `step_from_raw`,
//! - the built-in predictor enabled, so the motor stops early by the estimated
//!   in-flight mass instead of overshooting,
//! - an e-stop hook the core polls on every step,
//! - telemetry (`last_slope_ema_gps`, `last_inflight_g`, `early_stop_at_g`)
//!   printed at the end, which is what you tune `extra_latency_ms` against.
//!
//! Related examples:
//! - `quick_start` — the shipped simulation backend, minimal setup.
//! - `simulated_hardware` — implement the `Scale`/`Motor` traits yourself.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use doser_core::mocks::NoopScale;
use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, PredictorCfg, Timeouts};
use doser_traits::Motor;

/// ADC counts per gram for the emulated rig.
const COUNTS_PER_GRAM: f32 = 100.0;
/// Grams delivered per motor step.
const GRAMS_PER_STEP: f32 = 0.004;
/// Control rate. `step_from_raw` paces itself by one period, so this is also
/// how much wall-clock (and simulated) time one outer-loop tick covers.
const SAMPLE_RATE_HZ: u32 = 200;
const TICK: Duration = Duration::from_millis(1000 / SAMPLE_RATE_HZ as u64);
/// Raw samples averaged per control step.
const OVERSAMPLE: u32 = 4;
/// Ticks a delivered parcel of beans spends in the air before it lands. This is
/// the transport lag the predictor exists to cancel.
const FLIGHT_TICKS: usize = 6;

/// Records what the control loop commands, so the outer loop can model the
/// resulting flow. Stands in for the real stepper driver.
struct ProbeMotor {
    sps: Arc<AtomicU32>,
    running: Arc<AtomicBool>,
}

impl Motor for ProbeMotor {
    fn set_speed(
        &mut self,
        steps_per_sec: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.sps.store(steps_per_sec, Ordering::Release);
        Ok(())
    }

    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.running.store(true, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.sps.store(0, Ordering::Release);
        self.running.store(false, Ordering::Release);
        Ok(())
    }
}

/// Beans already in the air when the motor stops still land in the cup.
/// Deliveries are queued for `FLIGHT_TICKS` ticks before they reach the scale.
#[derive(Default)]
struct Hopper {
    on_scale_g: f32,
    in_flight: [f32; FLIGHT_TICKS],
    head: usize,
}

impl Hopper {
    /// Advance one tick: land the oldest parcel, queue a new one, report mass.
    fn tick(&mut self, delivered_g: f32) -> f32 {
        self.on_scale_g += self.in_flight[self.head];
        self.in_flight[self.head] = delivered_g;
        self.head = (self.head + 1) % FLIGHT_TICKS;
        self.on_scale_g
    }
}

fn main() -> Result<(), eyre::Report> {
    let sps = Arc::new(AtomicU32::new(0));
    let running = Arc::new(AtomicBool::new(false));
    let estop = Arc::new(AtomicBool::new(false));

    let motor = ProbeMotor {
        sps: Arc::clone(&sps),
        running: Arc::clone(&running),
    };

    let estop_probe = Arc::clone(&estop);
    let target_g = 5.0_f32;

    let mut doser = Doser::builder()
        // The core never reads this scale: we drive it with `step_from_raw`.
        // `NoopScale` makes that explicit — it errors if anyone calls `step()`.
        .with_scale(NoopScale)
        .with_motor(motor)
        .with_filter(FilterCfg {
            median_window: 3,
            sample_rate_hz: SAMPLE_RATE_HZ,
            ..FilterCfg::default()
        })
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts::default())
        .with_predictor(PredictorCfg {
            enabled: true,
            window: 8,
            // Lag the predictor must compensate for, on top of the loop period
            // it derives from `sample_rate_hz`.
            extra_latency_ms: FLIGHT_TICKS as u64 * TICK.as_millis() as u64,
            min_progress_ratio: 0.25,
        })
        // Polled by the core on every step; two consecutive trues latch a stop.
        .with_estop_check(move || estop_probe.load(Ordering::Relaxed))
        .with_estop_debounce(2)
        .with_calibration_gain_offset(1.0 / COUNTS_PER_GRAM, 0.0)
        .with_target_grams(target_g)
        .build()?;

    println!("dosing {target_g:.2} g with a caller-owned sampling loop");
    doser.begin();

    // Bound the run so the example always terminates, even if a future change
    // to the control law stops converging. The core's max-runtime watchdog is
    // the real guard; this is belt and braces for an example.
    const MAX_TICKS: u32 = 20_000;
    let mut hopper = Hopper::default();
    let mut ticks = 0_u32;

    loop {
        ticks += 1;

        // ── The custom part: sample however you like, then hand over counts ──
        let commanded = if running.load(Ordering::Acquire) {
            sps.load(Ordering::Acquire)
        } else {
            0
        };
        let delivered_g = commanded as f32 * GRAMS_PER_STEP * TICK.as_secs_f32();
        let grams = hopper.tick(delivered_g);

        // Oversample the ADC and average. Each read carries ±1 count of noise,
        // which averaging (and the median prefilter above) removes.
        let mut acc = 0_i64;
        for i in 0..OVERSAMPLE {
            let noise = (i % 3) as i32 - 1;
            acc += i64::from((grams * COUNTS_PER_GRAM) as i32 + noise);
        }
        let raw = (acc / i64::from(OVERSAMPLE)) as i32;

        match doser.step_from_raw(raw)? {
            DosingStatus::Running => {}
            DosingStatus::Complete => {
                println!("complete: {:.2} g after {ticks} ticks", doser.last_weight());
                break;
            }
            DosingStatus::Aborted(e) => {
                println!("aborted after {ticks} ticks: {e}");
                break;
            }
        }

        if ticks >= MAX_TICKS {
            doser.motor_stop()?;
            eyre::bail!("strategy loop did not converge in {MAX_TICKS} ticks");
        }
    }

    // Telemetry the predictor exposes — what you tune `extra_latency_ms` against.
    if let Some(slope) = doser.last_slope_ema_gps() {
        println!("  final flow rate   : {slope:.3} g/s");
    }
    if let Some(inflight) = doser.last_inflight_g() {
        println!("  in-flight estimate: {inflight:.3} g");
    }
    match doser.early_stop_at_g() {
        Some(w) => println!("  predictor stopped the motor at {w:.2} g"),
        None => println!("  predictor never triggered (epsilon stop instead)"),
    }

    doser.motor_stop()?;
    Ok(())
}

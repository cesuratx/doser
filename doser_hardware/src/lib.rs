//! doser_hardware: hardware and simulation backends behind `doser_traits`.
//!
//! Features:
//! - `hardware`: enable Raspberry Pi GPIO/HX711-backed implementations.
//! - (default) no `hardware` feature: use simulation types that satisfy the traits.
//!
//! Note: The `rppal` dependency is optional and only enabled when the `hardware`
//!       feature is active. This lets CI on x86 build without pulling GPIO libs.

pub mod error;

// Make the HX711 driver module available when hardware feature is enabled.
#[cfg(feature = "hardware")]
mod hx711;

#[cfg(not(feature = "hardware"))]
pub mod sim {
    use doser_traits::{Motor, Scale};
    use std::error::Error;
    use std::time::Duration;

    /// Simple simulated scale that stores an internal weight in grams.
    /// `read()` returns a synthetic raw value (grams * 1000) as i32.
    #[derive(Default)]
    pub struct SimulatedScale {
        grams: f32,
    }

    impl SimulatedScale {
        pub fn new() -> Self {
            Self { grams: 0.0 }
        }

        /// Add grams to the simulated hopper (for tests/demos).
        pub fn push(&mut self, grams: f32) {
            self.grams += grams;
            if self.grams.is_sign_negative() {
                self.grams = 0.0;
            }
        }

        /// Set absolute weight (useful for deterministic tests).
        pub fn set(&mut self, grams: f32) {
            self.grams = grams.max(0.0);
        }
    }

    impl Scale for SimulatedScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            // Convert grams → a pretend "raw" reading. Core can treat this as grams if desired.
            Ok((self.grams * 1000.0) as i32)
        }
    }

    /// Minimal simulated motor; tracks speed and running state.
    #[derive(Default)]
    pub struct SimulatedMotor {
        speed_sps: u32,
        running: bool,
    }

    impl Motor for SimulatedMotor {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running = true;
            Ok(())
        }

        fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
            // You can enforce "must start first" semantics if you want:
            // if !self.running { return Err("motor not started".into()); }
            self.speed_sps = sps;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.speed_sps = 0;
            self.running = false;
            Ok(())
        }
    }
}

#[cfg(feature = "hardware")]
pub mod hardware {
    use crate::hx711::Hx711;
    use anyhow::{Context, Result};
    use doser_traits::{Motor, Scale};
    use rppal::gpio::{Gpio, OutputPin};
    use std::error::Error;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::Duration;
    use tracing::{info, warn};

    /// Hardware scale backed by HX711.
    pub struct HardwareScale {
        hx: Hx711,
    }

    impl HardwareScale {
        /// Create a new HX711-backed scale using DT and SCK GPIO pins.
        pub fn try_new(dt_pin: u8, sck_pin: u8) -> Result<Self> {
            let gpio = Gpio::new().context("open GPIO for HX711")?;
            let dt = gpio.get(dt_pin).context("get HX711 DT pin")?.into_input();
            let sck = gpio
                .get(sck_pin)
                .context("get HX711 SCK pin")?
                .into_output_low();
            // Channel A, gain = 128 uses 25 pulses after the 24-bit read.
            let hx = Hx711::new(dt, sck, 25)?;
            Ok(Self { hx })
        }

        /// Read a raw 24-bit value from HX711 with timeout.
        fn read_raw_timeout(
            &mut self,
            timeout: Duration,
        ) -> Result<i32, Box<dyn Error + Send + Sync>> {
            self.hx
                .read_with_timeout(timeout)
                .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(e) })
        }
    }

    impl Scale for HardwareScale {
        fn read(&mut self, timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            self.read_raw_timeout(timeout)
        }
    }

    /// Raspberry Pi step/dir motor driver with optional enable pin.
    pub struct HardwareMotor {
        dir: OutputPin,
        en: Option<OutputPin>,
        running: Arc<AtomicBool>,
        sps: Arc<AtomicU32>,
        handle: Option<JoinHandle<()>>,
        shutdown_tx: mpsc::Sender<()>,
    }

    impl HardwareMotor {
        /// Create a motor from GPIO pin numbers. EN is taken from the DOSER_EN_PIN env var if present.
        pub fn try_new(step_pin: u8, dir_pin: u8) -> Result<Self> {
            let en_env = std::env::var("DOSER_EN_PIN")
                .ok()
                .and_then(|s| s.parse::<u8>().ok());
            Self::try_new_with_en(step_pin, dir_pin, en_env)
        }

        /// Create a motor from GPIO pin numbers with an optional enable pin.
        /// Note: On A4988/DRV8825, EN is active-low (low = enabled). We default to disabled (high).
        pub fn try_new_with_en(step_pin: u8, dir_pin: u8, en_pin: Option<u8>) -> Result<Self> {
            let gpio = Gpio::new().context("open GPIO")?;
            let mut step = gpio
                .get(step_pin)
                .context("get STEP pin")?
                .into_output_low();
            let dir = gpio.get(dir_pin).context("get DIR pin")?.into_output_low();

            let en = match en_pin {
                Some(pin) => Some(gpio.get(pin).context("get EN pin")?.into_output_high()), // high = disabled
                None => None,
            };

            let running = Arc::new(AtomicBool::new(false));
            let sps = Arc::new(AtomicU32::new(0));
            let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

            let running_bg = running.clone();
            let sps_bg = sps.clone();
            // Move STEP into the background thread; not used elsewhere.
            let handle = thread::spawn(move || {
                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }
                    let is_running = running_bg.load(Ordering::Relaxed);
                    let sps_val = sps_bg.load(Ordering::Relaxed).clamp(0, 5_000);
                    if is_running && sps_val > 0 {
                        let period_us = 1_000_000u32 / sps_val; // total period in microseconds
                        let half = (period_us / 2).max(1);
                        // Rising edge
                        let _ = step.set_high();
                        spin_delay_min();
                        // High hold
                        spin_sleep_us(half as u64);
                        // Falling edge
                        let _ = step.set_low();
                        spin_delay_min();
                        // Low hold
                        spin_sleep_us(half as u64);
                    } else {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            });

            let mut motor = Self {
                dir,
                en,
                running,
                sps,
                handle: Some(handle),
                shutdown_tx,
            };
            // Default: disabled
            let _ = motor.set_enabled(false);
            Ok(motor)
        }

        /// Set direction: true = clockwise (DIR high), false = counterclockwise (DIR low)
        pub fn set_direction(&mut self, clockwise: bool) {
            if clockwise {
                let _ = self.dir.set_high();
            } else {
                let _ = self.dir.set_low();
            }
        }

        /// Enable or disable the driver (active-low enable pin, if present)
        pub fn set_enabled(&mut self, enabled: bool) -> Result<()> {
            if let Some(en) = self.en.as_mut() {
                if enabled {
                    en.set_low();
                } else {
                    en.set_high();
                }
            }
            Ok(())
        }

        /// Set speed in steps-per-second; worker thread reads this atomically.
        pub fn set_speed_sps(&mut self, sps: u32) {
            self.sps.store(sps, Ordering::Relaxed);
        }
    }

    impl Drop for HardwareMotor {
        fn drop(&mut self) {
            let _ = self.shutdown_tx.send(());
            self.running.store(false, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            // Disable on drop
            let _ = self.set_enabled(false);
        }
    }

    impl Motor for HardwareMotor {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.set_enabled(true)
                .map_err(|e| Box::<dyn Error + Send + Sync>::from(e))?;
            self.running.store(true, Ordering::Relaxed);
            info!("motor started");
            Ok(())
        }

        fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
            let clamped = sps.clamp(0, 5_000);
            if clamped == 0 {
                warn!("requested 0 sps; motor will idle");
            }
            self.set_speed_sps(clamped);
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running.store(false, Ordering::Relaxed);
            self.set_speed_sps(0);
            info!("motor stopped");
            Ok(())
        }
    }

    /// Very small spin to make edges clean.
    #[inline(always)]
    fn spin_delay_min() {
        std::hint::spin_loop();
    }

    /// Sleep for microseconds using std; coarse but sufficient for <= 5 kHz.
    fn spin_sleep_us(us: u64) {
        std::thread::sleep(Duration::from_micros(us));
    }

    /// E-stop checker: on ARM, read from a GPIO and expose as closure.
    pub fn make_estop_checker(
        pin: u8,
        active_low: bool,
        poll_ms: u64,
    ) -> Result<Box<dyn Fn() -> bool + Send + Sync>> {
        use std::sync::atomic::AtomicBool;
        let gpio = Gpio::new().context("open GPIO")?;
        let pin = gpio.get(pin).context("get E-STOP pin")?.into_input();
        let flag = Arc::new(AtomicBool::new(false));
        let flag_bg = flag.clone();
        thread::spawn(move || {
            loop {
                let level_low = pin.read() == rppal::gpio::Level::Low;
                let active = if active_low { level_low } else { !level_low };
                flag_bg.store(active, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(poll_ms.max(1)));
            }
        });
        Ok(Box::new(move || flag.load(Ordering::Relaxed)))
    }
}

// Re-exports for callers (CLI/tests) to pick the right backend easily.
#[cfg(not(feature = "hardware"))]
pub use sim::{SimulatedMotor, SimulatedScale};

#[cfg(feature = "hardware")]
pub use hardware::{HardwareMotor, HardwareScale, make_estop_checker};

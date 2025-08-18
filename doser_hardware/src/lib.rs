//! doser_hardware: hardware and simulation backends behind `doser_traits`.
//!
//! Features:
//! - `hardware`: enable Raspberry Pi GPIO/HX711-backed implementations.
//! - (default) no `hardware` feature: use simulation types that satisfy the traits.
//!
//! Note: The `rppal` dependency is optional and only enabled when the `hardware`
//!       feature is active. This lets CI on x86 build without pulling GPIO libs.

pub mod error;

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
    use anyhow::{Context, Result};
    use doser_traits::{Motor, Scale};
    use rppal::gpio::{Gpio, OutputPin};
    use std::error::Error;
    use std::sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};
    use tracing::{info, warn};

    /// Simple error wrapper to avoid unwraps in hardware paths.
    #[derive(Debug)]
    struct HwErr(&'static str);
    impl std::fmt::Display for HwErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "hardware error: {}", self.0)
        }
    }
    impl Error for HwErr {}

    /// Hardware scale backed by HX711 (skeleton).
    pub struct HardwareScale;
    impl HardwareScale {
        pub fn try_new(_dt_pin: u8, _sck_pin: u8) -> Result<Self> {
            // TODO: integrate actual HX711 driver
            Ok(Self)
        }
        fn read_raw_timeout(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            Ok(0)
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
        /// Create a motor from GPIO pin numbers. EN is optional (active-low enable).
        pub fn try_new(step_pin: u8, dir_pin: u8) -> Result<Self> {
            let gpio = Gpio::new().context("open GPIO")?;
            let mut step = gpio.get(step_pin).context("get STEP pin")?.into_output_low();
            let dir = gpio.get(dir_pin).context("get DIR pin")?.into_output_low();

            // Optional enable pin: read from env or leave None; adjust as you wire config.
            let en = match std::env::var("DOSER_EN_PIN").ok().and_then(|s| s.parse::<u8>().ok()) {
                Some(en_pin) => Some(gpio.get(en_pin).context("get EN pin")?.into_output_high()), // high = disabled
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
                    if shutdown_rx.try_recv().is_ok() { break; }
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
            if clockwise { let _ = self.dir.set_high(); } else { let _ = self.dir.set_low(); }
        }

        /// Enable or disable the driver (active-low enable pin, if present)
        pub fn set_enabled(&mut self, enabled: bool) -> Result<()> {
            if let Some(en) = self.en.as_mut() {
                if enabled { en.set_low(); } else { en.set_high(); }
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
            self.set_enabled(true).map_err(|e| Box::<dyn Error + Send + Sync>::from(e))?;
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
    fn spin_delay_min() { std::hint::spin_loop(); }

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
        thread::spawn(move || loop {
            let level_low = pin.read() == rppal::gpio::Level::Low;
            let active = if active_low { level_low } else { !level_low };
            flag_bg.store(active, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(poll_ms.max(1)));
        });
        Ok(Box::new(move || flag.load(Ordering::Relaxed)))
    }
}

// Re-exports for callers (CLI/tests) to pick the right backend easily.
#[cfg(not(feature = "hardware"))]
pub use sim::{SimulatedMotor, SimulatedScale};

#[cfg(feature = "hardware")]
pub use hardware::{HardwareMotor, HardwareScale, make_estop_checker};

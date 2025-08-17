//! doser_hardware: hardware and simulation backends behind `doser_traits`.
//!
//! Features:
//! - `hardware`: enable Raspberry Pi GPIO/HX711-backed implementations.
//! - (default) no `hardware` feature: use simulation types that satisfy the traits.

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
    use doser_traits::{Motor, Scale};
    use std::error::Error;
    use std::fmt::{Display, Formatter};
    use std::time::{Duration, Instant};

    // Bring in rppal for actual GPIO when you wire it up.
    // use rppal::gpio::{Gpio, InputPin, OutputPin};

    /// Simple error wrapper to avoid unwraps in hardware paths.
    #[derive(Debug)]
    struct HwErr(String);
    impl Display for HwErr {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "hardware error: {}", self.0)
        }
    }
    impl Error for HwErr {}

    /// Hardware scale backed by HX711 (skeleton).
    /// Fill in pin types and HX711 driver hookup inside.
    pub struct HardwareScale {
        // dt: InputPin,
        // sck: OutputPin,
        // gain_pulses: u8,
        // offset: i32,
        // scale_factor: f32,
    }

    impl HardwareScale {
        /// Fallible constructor — never unwrap.
        pub fn try_new(
            _dt_pin: u8,
            _sck_pin: u8,
            // _gain_pulses: u8,
        ) -> Result<Self, Box<dyn Error + Send + Sync>> {
            // let gpio = Gpio::new().map_err(|e| HwErr(format!("gpio: {e}")))?;
            // let dt = gpio.get(_dt_pin).map_err(|e| HwErr(format!("dt pin: {e}")))?.into_input();
            // let sck = gpio.get(_sck_pin).map_err(|e| HwErr(format!("sck pin: {e}")))?.into_output();
            Ok(Self {
                // dt,
                // sck,
                // gain_pulses: _gain_pulses,
                // offset: 0,
                // scale_factor: 1.0,
            })
        }

        /// Example of a timed raw read. Replace body with your HX711 read logic.
        fn read_raw_timeout(
            &mut self,
            timeout: Duration,
        ) -> Result<i32, Box<dyn Error + Send + Sync>> {
            let _deadline = Instant::now() + timeout;
            // Wait for DRDY (DT low), then clock out 24 bits, sign-extend.
            // For now, return a stub value so the type-check passes:
            Ok(0)
        }
    }

    impl Scale for HardwareScale {
        fn read(&mut self, timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            // If you keep a scale_factor/offset internally, apply here; otherwise
            // return raw i32 and let core interpret it.
            self.read_raw_timeout(timeout)
        }
    }

    /// Hardware motor backed by your step/dir driver (skeleton).
    pub struct HardwareMotor {
        // step: OutputPin,
        // dir: OutputPin,
        running: bool,
        speed_sps: u32,
    }

    impl HardwareMotor {
        /// Fallible constructor — never unwrap.
        pub fn try_new(_step_pin: u8, _dir_pin: u8) -> Result<Self, Box<dyn Error + Send + Sync>> {
            // let gpio = Gpio::new().map_err(|e| HwErr(format!("gpio: {e}")))?;
            // let step = gpio.get(_step_pin).map_err(|e| HwErr(format!("step pin: {e}")))?.into_output();
            // let dir  = gpio.get(_dir_pin ).map_err(|e| HwErr(format!("dir pin: {e}")))?.into_output();
            Ok(Self {
                // step,
                // dir,
                running: false,
                speed_sps: 0,
            })
        }
    }

    impl Motor for HardwareMotor {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            // Enable your driver if needed
            self.running = true;
            Ok(())
        }

        fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
            // Program your stepper pulse timing here
            self.speed_sps = sps;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            // Disable driver / stop pulsing
            self.speed_sps = 0;
            self.running = false;
            Ok(())
        }
    }

    #[cfg(feature = "hardware")]
    #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
    pub fn make_estop_checker(
        pin: u8,
        active_low: bool,
        poll_ms: u64,
    ) -> Result<Box<dyn Fn() -> bool + Send + Sync>, Box<dyn std::error::Error + Send + Sync>> {
        use rppal::gpio::Gpio;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::thread;
        use std::time::Duration;

        let gpio = Gpio::new()?;
        let pin = gpio.get(pin)?.into_input();
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

    #[cfg(feature = "hardware")]
    #[cfg(not(any(target_arch = "arm", target_arch = "aarch64")))]
    pub fn make_estop_checker(
        _pin: u8,
        _active_low: bool,
        _poll_ms: u64,
    ) -> Result<Box<dyn Fn() -> bool + Send + Sync>, Box<dyn std::error::Error + Send + Sync>> {
        // Non-ARM platforms: return a stub that never trips.
        Ok(Box::new(|| false))
    }
}

// Re-exports for callers (CLI/tests) to pick the right backend easily.
#[cfg(not(feature = "hardware"))]
pub use sim::{SimulatedMotor, SimulatedScale};

#[cfg(feature = "hardware")]
pub use hardware::{HardwareMotor, HardwareScale};

#[cfg(feature = "hardware")]
pub use hardware::make_estop_checker;

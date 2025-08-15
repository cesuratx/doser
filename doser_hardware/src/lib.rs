#[cfg(feature = "hardware")]
pub mod hx711;
use doser_traits::{Motor, Scale};
use std::cell::Cell;
use std::rc::Rc;

/// Simulated scale implementation
pub struct SimulatedScale {
    weight: Rc<Cell<f32>>,
    scale_factor: Rc<Cell<f32>>,
}

impl SimulatedScale {
    pub fn new() -> Self {
        SimulatedScale {
            weight: Rc::new(Cell::new(0.0)),
            scale_factor: Rc::new(Cell::new(1.0)),
        }
    }
}

impl Scale for SimulatedScale {
    fn read(
        &mut self,
        _timeout: std::time::Duration,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        let w = self.weight.get() + 2.0;
        self.weight.set(w);
        let factor = self.scale_factor.get();
        println!("Reading scale (simulated): {:.2}g", w * factor);
        Ok((w * factor) as i32)
    }
}

/// Simulated motor implementation
pub struct SimulatedMotor;

impl SimulatedMotor {
    pub fn start(&mut self) {
        println!("Motor started (simulated)");
    }
}

impl Motor for SimulatedMotor {
    fn set_speed(
        &mut self,
        _steps_per_sec: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Motor started (simulated)");
        Ok(())
    }
}

#[cfg(feature = "hardware")]
use hx711::Hx711;

#[cfg(feature = "hardware")]
pub struct HardwareScale {
    hx711: hx711::Hx711,
    scale_factor: f32,
}

#[cfg(feature = "hardware")]
impl HardwareScale {
    pub fn new(dt_pin: u8, sck_pin: u8, gain_pulses: u8) -> Result<Self, hx711::HwError> {
        let hx711 = hx711::Hx711::new(dt_pin, sck_pin, gain_pulses)?;
        Ok(HardwareScale {
            hx711,
            scale_factor: 1.0,
        })
    }
}

#[cfg(feature = "hardware")]
impl Scale for HardwareScale {
    fn read(&mut self, timeout: std::time::Duration) -> Result<i32, crate::error::HwError> {
        let mut attempts = 0;
        let max_attempts = 3;
        loop {
            match self.hx711.read_with_timeout(timeout) {
                Ok(raw) => {
                    tracing::debug!(raw = raw, "hx711 sample");
                    return Ok(raw);
                }
                Err(crate::error::HwError::Timeout) if attempts < max_attempts => {
                    attempts += 1;
                    tracing::warn!(retries = attempts, "scale timeout, retrying");
                }
                Err(e) => {
                    tracing::error!("Scale read error: {}", e);
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(feature = "hardware")]
use stepper::Stepper;

#[cfg(feature = "hardware")]
pub struct HardwareMotor {
    stepper: Stepper,
}

#[cfg(feature = "hardware")]
impl HardwareMotor {
    pub fn new(step_pin: u8, dir_pin: u8) -> Self {
        HardwareMotor {
            stepper: Stepper::new(step_pin, dir_pin),
        }
    }
}

#[cfg(feature = "hardware")]
impl Motor for HardwareMotor {
    fn set_speed(&mut self, steps_per_sec: u32) -> Result<(), crate::error::HwError> {
        self.stepper.set_speed(steps_per_sec);
        Ok(())
    }
    fn stop(&mut self) -> Result<(), crate::error::HwError> {
        self.stepper.stop();
        Ok(())
    }
    fn start(&mut self) -> Result<(), crate::error::HwError> {
        self.stepper.start();
        Ok(())
    }
}

#[cfg(feature = "hardware")]
mod stepper {
    use rppal::gpio::{Gpio, OutputPin};
    use std::thread::sleep;
    use std::time::Duration;

    pub struct Stepper {
        step: OutputPin,
        dir: OutputPin,
    }

    impl Stepper {
        pub fn new(step_pin: u8, dir_pin: u8) -> Self {
            let gpio = Gpio::new().unwrap();
            let step = gpio.get(step_pin).unwrap().into_output();
            let dir = gpio.get(dir_pin).unwrap().into_output();
            Stepper { step, dir }
        }

        pub fn set_speed(&mut self, _steps_per_sec: u32) {
            // Implement speed control if needed
        }

        pub fn start(&mut self) {
            self.dir.set_high(); // Example: set direction
            println!("Stepper motor started (hardware)");
            // Example: pulse step pin for a short time
            for _ in 0..100 {
                self.step.set_high();
                sleep(Duration::from_micros(500));
                self.step.set_low();
                sleep(Duration::from_micros(500));
            }
        }

        pub fn stop(&mut self) {
            println!("Stepper motor stopped (hardware)");
            // Optionally set pins low
            self.step.set_low();
            self.dir.set_low();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simulated_scale() {
        let mut scale = SimulatedScale::new();
        let w1 = scale.read(std::time::Duration::from_millis(100)).unwrap();
        let w2 = scale.read(std::time::Duration::from_millis(100)).unwrap();
        assert!(w2 > w1);
    }

    #[test]
    fn test_simulated_motor() {
        let mut motor = SimulatedMotor;
        motor.start();
        motor.stop();
    }
}

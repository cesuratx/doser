#[cfg(feature = "hardware")]
mod hx711 {
    use rppal::gpio::{Gpio, InputPin, OutputPin};
    use std::thread::sleep;
    use std::time::Duration;

    pub struct HX711 {
        dt: InputPin,
        sck: OutputPin,
        offset: i32,
    }

    impl HX711 {
        pub fn new(dt_pin: u8, sck_pin: u8) -> Self {
            let gpio = Gpio::new().unwrap();
            let dt = gpio.get(dt_pin).unwrap().into_input();
            let sck = gpio.get(sck_pin).unwrap().into_output();
            HX711 { dt, sck, offset: 0 }
        }

        pub fn read_raw(&mut self) -> i32 {
            // Basic HX711 reading algorithm
            while self.dt.is_high() {}
            let mut count = 0i32;
            for _ in 0..24 {
                self.sck.set_high();
                sleep(Duration::from_micros(1));
                count <<= 1;
                self.sck.set_low();
                sleep(Duration::from_micros(1));
                if self.dt.is_high() {
                    count += 1;
                }
            }
            self.sck.set_high();
            count ^= 0x800000;
            self.sck.set_low();
            count
        }

        pub fn tare(&mut self) {
            self.offset = self.read_raw();
        }

        pub fn get_weight(&mut self, scale: f32) -> f32 {
            let raw = self.read_raw();
            ((raw - self.offset) as f32) / scale
        }
    }
}
use std::cell::Cell;
use std::rc::Rc;

/// Scale trait for abstraction
pub trait Scale {
    fn tare(&mut self);
    fn calibrate(&mut self, known_weight: f32);
    fn read_weight(&mut self) -> f32;
}

/// Motor trait for abstraction
pub trait Motor {
    fn start(&mut self);
    fn stop(&mut self);
}

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
    fn tare(&mut self) {
        self.weight.set(0.0);
        println!("Scale tared (simulated)");
    }
    fn calibrate(&mut self, known_weight: f32) {
        let raw = self.weight.get();
        let factor = if raw > 0.0 { known_weight / raw } else { 1.0 };
        self.scale_factor.set(factor);
        println!("Calibration complete. Scale factor set to {:.4}", factor);
    }
    fn read_weight(&mut self) -> f32 {
        let w = self.weight.get() + 2.0;
        self.weight.set(w);
        let factor = self.scale_factor.get();
        println!("Reading scale (simulated): {:.2}g", w * factor);
        w * factor
    }
}

/// Simulated motor implementation
pub struct SimulatedMotor;

impl Motor for SimulatedMotor {
    fn start(&mut self) {
        println!("Motor started (simulated)");
    }
    fn stop(&mut self) {
        println!("Motor stopped (simulated)");
    }
}

#[cfg(feature = "hardware")]
use hx711::HX711;

#[cfg(feature = "hardware")]
pub struct HardwareScale {
    hx711: HX711,
    scale_factor: f32,
}

#[cfg(feature = "hardware")]
impl HardwareScale {
    pub fn new(dt_pin: u8, sck_pin: u8) -> Self {
        HardwareScale {
            hx711: HX711::new(dt_pin, sck_pin),
            scale_factor: 1.0,
        }
    }
}

#[cfg(feature = "hardware")]
impl Scale for HardwareScale {
    fn tare(&mut self) {
        self.hx711.tare();
        println!("Scale tared (hardware)");
    }
    fn calibrate(&mut self, known_weight: f32) {
        let raw = self.hx711.read_raw() as f32;
        self.scale_factor = if raw > 0.0 { known_weight / raw } else { 1.0 };
        println!(
            "Calibration complete. Scale factor set to {:.4}",
            self.scale_factor
        );
    }
    fn read_weight(&mut self) -> f32 {
        let raw = self.hx711.read_raw() as f32;
        let weight = raw * self.scale_factor;
        println!("Reading scale (hardware): {:.2}g", weight);
        weight
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
    fn start(&mut self) {
        self.stepper.start();
    }
    fn stop(&mut self) {
        self.stepper.stop();
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
        scale.tare();
        scale.calibrate(10.0);
        let w1 = scale.read_weight();
        let w2 = scale.read_weight();
        assert!(w2 > w1);
    }

    #[test]
    fn test_simulated_motor() {
        let mut motor = SimulatedMotor;
        motor.start();
        motor.stop();
    }
}

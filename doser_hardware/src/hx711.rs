use std::time::{Duration, Instant};
use tracing::trace;

use crate::error::{HwError, Result};

pub struct Hx711 {
    dt: rppal::gpio::InputPin,
    sck: rppal::gpio::OutputPin,
    gain_pulses: u8, // 25, 26, 27 based on gain/channel
}

impl Hx711 {
    pub fn new(
        dt_pin: rppal::gpio::InputPin,
        mut sck_pin: rppal::gpio::OutputPin,
        gain_pulses: u8,
    ) -> Result<Self> {
        sck_pin.set_low(); // clock idle low
        Ok(Self {
            dt: dt_pin,
            sck: sck_pin,
            gain_pulses,
        })
    }

    pub fn read_with_timeout(&mut self, timeout: Duration) -> Result<i32> {
        let deadline = Instant::now() + timeout;

        // Wait for data ready (DT goes low)
        while self.dt.is_high() {
            if Instant::now() >= deadline {
                return Err(HwError::Timeout);
            }
            std::thread::sleep(Duration::from_micros(200));
        }

        // Clock out 24 bits
        let mut value: i32 = 0;
        for _ in 0..24 {
            self.sck.set_high();
            // short, consistent timing
            spin_delay_100ns();
            value = (value << 1) | if self.dt.is_high() { 1 } else { 0 };
            self.sck.set_low();
            spin_delay_100ns();
        }

        // Pulse gain to set next measurement
        for _ in 0..self.gain_pulses {
            self.sck.set_high();
            spin_delay_100ns();
            self.sck.set_low();
            spin_delay_100ns();
        }

        // Sign extend 24-bit
        if (value & 0x800000) != 0 {
            value |= !0xFFFFFF;
        }
        trace!(raw = value, "hx711 raw read");
        Ok(value)
    }
}

#[inline(always)]
fn spin_delay_100ns() {
    // Do nothing; a few CPU cycles—tweak if needed.
    std::hint::spin_loop();
}

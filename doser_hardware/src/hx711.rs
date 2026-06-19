use std::time::Duration;
use tracing::trace;

use crate::error::Result;
use crate::util::{busy_wait_min_1us, wait_until_low_with_timeout};
use doser_traits::clock::MonotonicClock;

pub struct Hx711 {
    dt: rppal::gpio::InputPin,
    sck: rppal::gpio::OutputPin,
    // Extra SCK pulses sent after the 24 data bits; they select the next
    // conversion's gain/channel: 1 = ch A/gain 128, 2 = ch B/gain 32,
    // 3 = ch A/gain 64 (i.e. 25, 26, or 27 total pulses per read).
    gain_pulses: u8,
    data_ready_timeout: Duration,
}

impl Hx711 {
    pub fn new(
        dt_pin: rppal::gpio::InputPin,
        mut sck_pin: rppal::gpio::OutputPin,
        gain_pulses: u8,
        data_ready_timeout: Duration,
    ) -> Result<Self> {
        sck_pin.set_low(); // clock idle low
        Ok(Self {
            dt: dt_pin,
            sck: sck_pin,
            gain_pulses,
            data_ready_timeout,
        })
    }

    pub fn read_with_timeout(&mut self, timeout: Duration) -> Result<i32> {
        // Use the smaller of the per-call timeout and configured data-ready timeout
        let eff = if timeout < self.data_ready_timeout {
            timeout
        } else {
            self.data_ready_timeout
        };

        // Wait for data ready (DT goes low) with micro-sleeps
        let clock = MonotonicClock::new();
        wait_until_low_with_timeout(
            || self.dt.is_high(),
            eff,
            Duration::from_micros(200),
            &clock,
        )?;

        // Clock out 24 bits. The HX711 requires SCK high/low times ≥ ~0.2µs and
        // samples DT while SCK is high, so each edge is followed by a ~1µs busy-wait.
        let mut value: i32 = 0;
        for _ in 0..24 {
            self.sck.set_high();
            busy_wait_min_1us();
            value = (value << 1) | i32::from(self.dt.is_high());
            self.sck.set_low();
            busy_wait_min_1us();
        }

        // Send the extra gain/channel pulses. Combined with the 24 data pulses
        // above this gives 24 + gain_pulses = 25/26/27 total, selecting the
        // gain/channel for the next conversion.
        for _ in 0..self.gain_pulses {
            self.sck.set_high();
            busy_wait_min_1us();
            self.sck.set_low();
            busy_wait_min_1us();
        }

        // Sign extend 24-bit
        if (value & 0x800000) != 0 {
            value |= !0xFFFFFF;
        }
        trace!(raw = value, "hx711 raw read");
        Ok(value)
    }
}

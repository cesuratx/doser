//! Standalone HX711 hardware probe (diagnostic).
//!
//! Bit-bangs the HX711 directly with accurate microsecond timing and verbose
//! output, independent of the production driver. Use it to tell apart a wiring
//! fault, an unpowered chip, a missing load cell, and a timing problem.
//!
//! Run on the Pi:
//!   cargo run -p doser_hardware --example hx711_probe --features hardware
//!
//! Optional env overrides (BCM numbering):
//!   HX711_DT=5 HX711_SCK=6 HX711_HIGH_US=2 cargo run ... --example hx711_probe --features hardware

use rppal::gpio::{Gpio, Level};
use std::time::{Duration, Instant};

/// Busy-wait an accurate number of microseconds (Instant-based, no syscalls).
fn busy_us(us: u64) {
    if us == 0 {
        return;
    }
    let start = Instant::now();
    let dur = Duration::from_micros(us);
    while start.elapsed() < dur {
        std::hint::spin_loop();
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dt_pin = env_u64("HX711_DT", 5) as u8;
    let sck_pin = env_u64("HX711_SCK", 6) as u8;
    let high_us = env_u64("HX711_HIGH_US", 2); // SCK high time
    let low_us = env_u64("HX711_LOW_US", 2); // SCK low time
    let gain_pulses = env_u64("HX711_GAIN_PULSES", 1) as u32; // 1 => 25 total
    let reads = env_u64("HX711_READS", 10) as u32; // number of reads (streaming)
    let stream = std::env::var("HX711_STREAM").is_ok(); // compact min/max tracking

    println!(
        "HX711 probe: DT=GPIO{dt_pin} SCK=GPIO{sck_pin} high={high_us}us low={low_us}us \
         total_pulses={}",
        24 + gain_pulses
    );

    let gpio = Gpio::new()?;
    let dt = gpio.get(dt_pin)?.into_input();
    let mut sck = gpio.get(sck_pin)?.into_output_low();

    // Wake from any power-down: SCK low, give the chip time to produce a sample.
    sck.set_low();
    std::thread::sleep(Duration::from_millis(60));

    let mut vmin = i32::MAX;
    let mut vmax = i32::MIN;
    for read_idx in 0..reads {
        // Wait for data ready (DT low), measuring how long it stays high.
        let wait_start = Instant::now();
        let mut timed_out = false;
        while dt.read() == Level::High {
            if wait_start.elapsed() > Duration::from_millis(400) {
                timed_out = true;
                break;
            }
            busy_us(50);
        }
        let ready_wait = wait_start.elapsed();

        if timed_out {
            println!(
                "read {read_idx}: TIMEOUT waiting for DT low after {:?} (DT stuck HIGH)",
                ready_wait
            );
            continue;
        }

        // Clock out 24 bits, MSB first; sample DT while SCK is high.
        let mut value: i32 = 0;
        let mut bits = String::new();
        for _ in 0..24 {
            sck.set_high();
            busy_us(high_us);
            let bit = u8::from(dt.read() == Level::High);
            bits.push(if bit == 1 { '1' } else { '0' });
            value = (value << 1) | i32::from(bit);
            sck.set_low();
            busy_us(low_us);
        }
        // Gain/channel select pulses.
        for _ in 0..gain_pulses {
            sck.set_high();
            busy_us(high_us);
            sck.set_low();
            busy_us(low_us);
        }
        // Sign-extend 24-bit two's complement.
        if (value & 0x80_0000) != 0 {
            value |= !0xFF_FFFF;
        }

        // After a real read DT should return HIGH within ~12.5ms (80 SPS) and
        // then drop LOW for the next sample. Measure the HIGH duration.
        let after = Instant::now();
        let mut went_high = false;
        while after.elapsed() < Duration::from_millis(50) {
            if dt.read() == Level::High {
                went_high = true;
                break;
            }
            busy_us(50);
        }
        let high_after_us = after.elapsed().as_micros();

        if stream {
            let new_extreme = value < vmin || value > vmax;
            vmin = vmin.min(value);
            vmax = vmax.max(value);
            // Stay quiet while the value is flat; shout on any new extreme (wiring
            // change / load), and emit a heartbeat every ~50 reads so we know it's alive.
            if new_extreme {
                println!(
                    "read {read_idx:>4}: raw={value:>9}  *** MOVED ***  span={}  ({} .. {})",
                    vmax.saturating_sub(vmin),
                    vmin,
                    vmax
                );
            } else if read_idx % 50 == 0 {
                println!(
                    "read {read_idx:>4}: raw={value:>9}  (flat, span={})",
                    vmax.saturating_sub(vmin)
                );
            }
            continue;
        }

        println!(
            "read {read_idx}: raw={value:>9}  bits={bits}  ready_wait={ready_wait:?}  \
             DT_high_after_read={}",
            if went_high {
                format!("YES (~{high_after_us}us)")
            } else {
                "NO (stayed LOW)".to_string()
            }
        );
    }

    Ok(())
}

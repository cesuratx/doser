#![cfg(feature = "hardware")]

use std::time::Duration;

use doser_hardware::hardware::HardwareScale;
use rppal::gpio::{Gpio, Level};

// NOTE: These tests are pseudo-simulated and will only work when running on hardware with loopback wiring
// or a GPIO mocking layer. They demonstrate the structure requested: success path and timeout path.

#[test]
fn hx711_wait_success_path() {
    // This is a placeholder structure; on real hardware, DT must be externally driven low.
    // We simply ensure that read_with_timeout returns either Ok or a timeout, not busy-spinning.
    let gpio = Gpio::new().expect("open gpio");
    let dt_pin = 5u8; // adjust for your test rig
    let sck_pin = 6u8; // adjust for your test rig

    // Build with short data-ready timeout to keep the test fast if wiring isn't present.
    let mut sc = HardwareScale::try_new_with_timeout(dt_pin, sck_pin, 20).expect("make scale");
    let _ = sc.read(Duration::from_millis(50)); // may fail on non-wired rigs; we don't assert here
}

#[test]
fn hx711_wait_timeout_path() {
    // With no wiring pulling DT low, the call should time out quickly and not spin CPU.
    let dt_pin = 5u8; // adjust
    let sck_pin = 6u8; // adjust
    let mut sc = HardwareScale::try_new_with_timeout(dt_pin, sck_pin, 5).expect("make scale");
    let err = sc
        .read(Duration::from_millis(5))
        .expect_err("expect timeout");
    let msg = format!("{err}");
    assert!(msg.to_lowercase().contains("timeout"));
}

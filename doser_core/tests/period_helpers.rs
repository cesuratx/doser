// Focused tests for period helpers.
use doser_core::util::{period_ms, period_us};

#[test]
fn period_us_clamps_and_floors() {
    // hz=1 → 1s
    assert_eq!(period_us(1), 1_000_000);
    // hz=2 → 500_000µs
    assert_eq!(period_us(2), 500_000);
    // Very high hz floors to 1µs minimum
    assert_eq!(period_us(1_000_000), 1);
    assert_eq!(period_us(u32::MAX), 1);
}

#[test]
fn period_ms_minimum_and_resolution_note() {
    // hz=1 → 1000ms
    assert_eq!(period_ms(1), 1000);
    // hz=2 → 500ms
    assert_eq!(period_ms(2), 500);
    // hz>=1000 floors to 0ms but we cap to >=1ms
    assert_eq!(period_ms(1000), 1);
    assert_eq!(period_ms(10_000), 1);
    assert_eq!(period_ms(u32::MAX), 1);
}

// In debug builds we assert on hz=0 to catch misconfiguration early.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "sample_rate_hz must be > 0")]
fn period_us_panics_on_zero_hz_in_debug() {
    let _ = period_us(0);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "sample_rate_hz must be > 0")]
fn period_ms_panics_on_zero_hz_in_debug() {
    let _ = period_ms(0);
}

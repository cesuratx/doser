//! Common time/period helpers for doser_core.

/// Number of microseconds in one second.
pub const MICROS_PER_SEC: u64 = 1_000_000;
/// Number of milliseconds in one second.
pub const MILLIS_PER_SEC: u64 = 1_000;

/// Compute the period in microseconds for a given sampling rate in Hz.
/// - Clamps `hz` to at least 1 to avoid division by zero.
/// - Ensures result is at least 1 microsecond.
#[inline]
pub fn period_us(hz: u32) -> u64 {
    (MICROS_PER_SEC / u64::from(hz.max(1))).max(1)
}

/// Compute the period in milliseconds for a given sampling rate in Hz.
/// - Clamps `hz` to at least 1 to avoid division by zero.
/// - Ensures result is at least 1 millisecond.
#[inline]
pub fn period_ms(hz: u32) -> u64 {
    (MILLIS_PER_SEC / u64::from(hz.max(1))).max(1)
}

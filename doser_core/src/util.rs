//! Common time/period helpers for doser_core.

/// Compute the period in microseconds for a given sampling rate in Hz.
/// - Clamps `hz` to at least 1 to avoid division by zero.
/// - Ensures result is at least 1 microsecond.
#[inline]
pub fn period_us(hz: u32) -> u64 {
    (1_000_000u64 / u64::from(hz.max(1))).max(1)
}

/// Compute the period in milliseconds for a given sampling rate in Hz.
/// - Clamps `hz` to at least 1 to avoid division by zero.
/// - Ensures result is at least 1 millisecond.
#[inline]
pub fn period_ms(hz: u32) -> u64 {
    (1000u64 / u64::from(hz.max(1))).max(1)
}

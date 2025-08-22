//! Common time/period helpers for doser_core.

/// Number of microseconds in one second.
pub const MICROS_PER_SEC: u64 = 1_000_000;
/// Number of milliseconds in one second.
pub const MILLIS_PER_SEC: u64 = 1_000;

/// Compute the period in microseconds for a given sampling rate in Hz.
/// - Expects `hz > 0` (validated by higher layers); in debug builds we assert this.
/// - Clamps `hz` to at least 1 at runtime to avoid division by zero in release builds.
/// - Floors due to integer division and ensures the result is at least 1 microsecond.
///   For very high rates (e.g., ≥ 1_000_000 Hz), the computed period underflows to 0
///   and is floored to 1µs as the minimum representable unit.
#[inline]
pub fn period_us(hz: u32) -> u64 {
    debug_assert!(
        hz > 0,
        "sample_rate_hz must be > 0; use validation to enforce this"
    );
    (MICROS_PER_SEC / u64::from(hz.max(1))).max(1)
}

/// Compute the period in milliseconds for a given sampling rate in Hz.
/// - Expects `hz > 0` (validated by higher layers); in debug builds we assert this.
/// - Clamps `hz` to at least 1 at runtime to avoid division by zero in release builds.
/// - Floors due to integer division and ensures the result is at least 1 millisecond.
///   Note: This operates at millisecond resolution. For `hz ≥ 1000`, the true period
///   is < 1ms and will floor to 0; we cap to a minimum of 1ms. For accurate scheduling
///   at higher rates, use `period_us`.
#[inline]
pub fn period_ms(hz: u32) -> u64 {
    debug_assert!(
        hz > 0,
        "sample_rate_hz must be > 0; use validation to enforce this"
    );
    (MILLIS_PER_SEC / u64::from(hz.max(1))).max(1)
}

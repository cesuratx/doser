//! Common time/period helpers for doser_core.

/// Number of microseconds in one second.
pub const MICROS_PER_SEC: u64 = 1_000_000;
/// Number of milliseconds in one second.
pub const MILLIS_PER_SEC: u64 = 1_000;

/// Compute the period in microseconds for a given sampling rate in Hz.
/// - Requires `hz > 0` (validated by higher layers) and asserts this in all builds.
/// - Floors due to integer division and ensures the result is at least 1 microsecond.
///   For very high rates (e.g., ≥ 1_000_000 Hz), the computed period underflows to 0
///   and is floored to 1µs as the minimum representable unit.
#[inline]
pub fn period_us(hz: u32) -> u64 {
    assert!(hz > 0, "hz must be > 0; validate at callsite");
    (MICROS_PER_SEC / u64::from(hz)).max(1)
}

/// Compute the period in milliseconds for a given sampling rate in Hz.
/// - Requires `hz > 0` (validated by higher layers) and asserts this in all builds.
/// - Floors due to integer division and ensures the result is at least 1 millisecond.
///   Note: This operates at millisecond resolution. For `hz ≥ 1000`, the true period
///   is < 1ms and will floor to 0; we cap to a minimum of 1ms. For accurate scheduling
///   at higher rates, use `period_us`.
#[inline]
pub fn period_ms(hz: u32) -> u64 {
    assert!(hz > 0, "hz must be > 0; validate at callsite");
    (MILLIS_PER_SEC / u64::from(hz)).max(1)
}

/// Integer division rounded to nearest, with consistent behavior for negatives.
///
/// Behavior:
/// - For positive numerators, computes `(n + d/2) / d`.
/// - For negative numerators, computes `(n - d/2) / d`.
/// - Rust division truncates toward zero; this biasing yields round-to-nearest with
///   ties rounded away from zero (e.g., `-5/2 -> -3`, `5/2 -> 3`).
///
/// Parameters and constraints:
/// - `denom` must be strictly greater than 0. If `denom <= 0`, this function panics.
/// - Uses 64-bit intermediates, so no overflow occurs for any `i32` numerator and
///   positive `i32` denominator.
#[inline]
pub(crate) fn div_round_nearest_i32(numer: i32, denom: i32) -> i32 {
    assert!(denom > 0, "div_round_nearest_i32: denom must be > 0");
    let n = numer as i64;
    let d = denom as i64;
    let q = if n >= 0 {
        (n + (d / 2)) / d
    } else {
        (n - (d / 2)) / d
    };
    q as i32
}

#[cfg(test)]
mod rounding_tests {
    use super::div_round_nearest_i32;

    #[test]
    fn ties_away_from_zero() {
        assert_eq!(div_round_nearest_i32(5, 2), 3);
        assert_eq!(div_round_nearest_i32(-5, 2), -3);
        assert_eq!(div_round_nearest_i32(7, 3), 2);
        assert_eq!(div_round_nearest_i32(8, 3), 3);
    }

    #[test]
    fn handles_extremes_without_overflow() {
        assert_eq!(div_round_nearest_i32(i32::MAX, 2), (i32::MAX / 2) + 1);
        assert_eq!(div_round_nearest_i32(i32::MIN, 2), i32::MIN / 2);
    }
}

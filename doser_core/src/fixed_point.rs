//! Fixed-point centigram arithmetic helpers.
//!
//! Operating in centigrams (`i32`, 1 cg = 0.01 g) avoids per-sample floating-point
//! in the control loop and keeps all thresholds in a single integer unit.

/// Average of two i32 values, rounded to nearest with ties away from zero.
/// Uses 64-bit intermediates; cannot overflow.
#[inline]
pub fn avg2_round_nearest_i32(a: i32, b: i32) -> i32 {
    let s = (a as i64) + (b as i64);
    if s >= 0 {
        ((s + 1) / 2) as i32
    } else {
        ((s - 1) / 2) as i32
    }
}

/// Quantize a floating-point grams value to integer centigrams (cg), rounding to nearest
/// and clamping to the `i32` range. Non-finite values (NaN/±Inf) map to 0.
#[inline]
pub fn quantize_to_cg_i32(x_g: f32) -> i32 {
    if !x_g.is_finite() {
        return 0;
    }
    let scaled = (x_g * 100.0).round();
    if scaled >= i32::MAX as f32 {
        i32::MAX
    } else if scaled <= i32::MIN as f32 {
        i32::MIN
    } else {
        scaled as i32
    }
}

/// Absolute difference of two i32 values as u32 without overflow.
///
/// Uses 64-bit intermediates to avoid overflow during subtraction.
/// For any `i32` inputs, `|a - b| <= u32::MAX`, so the cast is always lossless.
#[inline]
pub fn abs_diff_i32_u32(a: i32, b: i32) -> u32 {
    let diff = (a as i64) - (b as i64);
    let mag = if diff >= 0 {
        diff as u64
    } else {
        (-diff) as u64
    };
    debug_assert!(
        mag <= u32::MAX as u64,
        "abs_diff_i32_u32: magnitude out of u32 range: {mag}"
    );
    mag as u32
}

/// Shorthand: convert grams (f32) to centigrams (i32) via rounding.
#[inline]
pub fn grams_to_cg(g: f32) -> i32 {
    ((g * 100.0).round()) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abs_diff_handles_extremes_losslessly() {
        let v = abs_diff_i32_u32(i32::MIN, i32::MAX);
        assert_eq!(v, u32::MAX);
    }

    #[test]
    fn abs_diff_simple_pairs() {
        assert_eq!(abs_diff_i32_u32(123, -456), 579);
        assert_eq!(abs_diff_i32_u32(-456, 123), 579);
        assert_eq!(abs_diff_i32_u32(0, 0), 0);
    }

    #[test]
    fn avg2_extremes_and_signs() {
        assert_eq!(avg2_round_nearest_i32(i32::MAX, i32::MAX), i32::MAX);
        assert_eq!(avg2_round_nearest_i32(i32::MIN, i32::MIN), i32::MIN);
        assert_eq!(avg2_round_nearest_i32(i32::MAX, i32::MIN), -1);
    }

    #[test]
    fn avg2_simple_pairs() {
        assert_eq!(avg2_round_nearest_i32(1, 2), 2);
        assert_eq!(avg2_round_nearest_i32(-1, 0), -1);
        assert_eq!(avg2_round_nearest_i32(10, 10), 10);
        assert_eq!(avg2_round_nearest_i32(-5, -6), -6);
    }
}

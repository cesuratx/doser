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

/// Fixed-point scaling applied to the calibration gain (centigrams-per-count)
/// before it is stored as an integer.
///
/// A plain `round(100 * gain_g_per_count)` (integer cg/count) destroys all
/// sub-centigram-per-count resolution. Realistic load cells have gains far below
/// 0.005 g/count (e.g. ~5e-4 g/count for ~100 g over a 24-bit ADC span), which
/// would quantize to `0` — making every reading collapse to the offset. Scaling
/// the gain by this factor keeps ~6 extra decimal digits of resolution while the
/// per-sample multiply stays integer (i128) and deterministic.
pub const GAIN_SCALE: i64 = 1_000_000;

/// Quantize a gain expressed in grams-per-count into a scaled integer of
/// `centigrams-per-count * GAIN_SCALE`, rounding to nearest and clamping to the
/// `i64` range. Non-finite inputs (NaN/±Inf) map to `0`.
#[inline]
pub fn gain_to_scaled_cg_per_count(gain_g_per_count: f32) -> i64 {
    if !gain_g_per_count.is_finite() {
        return 0;
    }
    // cg-per-count = 100 * g-per-count; scale up for fractional resolution.
    let scaled = ((gain_g_per_count as f64) * 100.0 * (GAIN_SCALE as f64)).round();
    if scaled >= i64::MAX as f64 {
        i64::MAX
    } else if scaled <= i64::MIN as f64 {
        i64::MIN
    } else {
        scaled as i64
    }
}

/// Convert a raw-count delta to centigrams using a scaled gain (see
/// [`GAIN_SCALE`]) plus an integer centigram offset, rounding to nearest with
/// ties away from zero and saturating to the `i32` range.
///
/// Uses an `i128` intermediate so the multiply cannot overflow for any `i64`
/// delta and gain.
#[inline]
pub fn cg_from_delta_scaled(
    delta_counts: i64,
    gain_scaled_cg_per_count: i64,
    offset_cg: i32,
) -> i32 {
    let num = (delta_counts as i128) * (gain_scaled_cg_per_count as i128);
    let den = GAIN_SCALE as i128;
    let half = den / 2;
    let q = if num >= 0 {
        (num + half) / den
    } else {
        (num - half) / den
    };
    let cg = q + (offset_cg as i128);
    if cg > i32::MAX as i128 {
        i32::MAX
    } else if cg < i32::MIN as i128 {
        i32::MIN
    } else {
        cg as i32
    }
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

    #[test]
    fn scaled_gain_preserves_small_gains() {
        // Sim/test gain: 0.01 g/count -> exactly 1 cg/count.
        assert_eq!(gain_to_scaled_cg_per_count(0.01), GAIN_SCALE);
        // Realistic load cell from the README example: 100 g over 182000 counts.
        let gain = 100.0_f32 / 182_000.0; // ~0.000549 g/count
        let scaled = gain_to_scaled_cg_per_count(gain);
        assert!(scaled > 0, "small gain must not quantize to zero");
        // 182000 counts from zero must read ~100.00 g (10000 cg), not 0.
        let cg = cg_from_delta_scaled(182_000, scaled, 0);
        assert!((9995..=10005).contains(&cg), "expected ~10000 cg, got {cg}");
        // Non-finite gain is treated as zero gain.
        assert_eq!(gain_to_scaled_cg_per_count(f32::NAN), 0);
        assert_eq!(gain_to_scaled_cg_per_count(f32::INFINITY), 0);
    }

    #[test]
    fn cg_from_delta_rounds_and_saturates() {
        // 1 cg/count, delta 123 -> 123 cg, plus offset.
        assert_eq!(cg_from_delta_scaled(123, GAIN_SCALE, 0), 123);
        assert_eq!(cg_from_delta_scaled(123, GAIN_SCALE, 7), 130);
        // Round to nearest, ties away from zero.
        assert_eq!(cg_from_delta_scaled(1, GAIN_SCALE / 2, 0), 1); // 0.5 -> 1
        assert_eq!(cg_from_delta_scaled(-1, GAIN_SCALE / 2, 0), -1); // -0.5 -> -1
        // Saturation on extreme gain/delta.
        assert_eq!(cg_from_delta_scaled(i64::MAX, i64::MAX, 0), i32::MAX);
        assert_eq!(cg_from_delta_scaled(i64::MIN, i64::MAX, 0), i32::MIN);
    }
}

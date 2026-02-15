//! Linear calibration from raw scale counts to grams / centigrams.
//!
//! The core representation uses centigrams (cg, 1 cg = 0.01 g) with `i32`
//! fixed-point arithmetic for deterministic, allocation-free control loop math.

use crate::fixed_point::quantize_to_cg_i32;

/// Simple linear calibration from raw scale counts to grams.
///
/// ```text
/// grams = gain_g_per_count * (raw - zero_counts) + offset_g
/// ```
#[derive(Debug, Clone)]
pub struct Calibration {
    pub gain_g_per_count: f32,
    pub zero_counts: i32,
    pub offset_g: f32,
}

impl Calibration {
    /// Convert raw counts to grams (floating-point path, used for display only).
    pub fn to_grams(&self, raw: i32) -> f32 {
        self.gain_g_per_count * ((raw - self.zero_counts) as f32) + self.offset_g
    }

    /// Convert raw counts directly to centigrams (cg, where 1 cg = 0.01 g)
    /// using integer fixed-point arithmetic.
    ///
    /// Definition (continuous):
    ///   grams = gain_g_per_count * (raw - zero_counts) + offset_g
    ///   centigrams = round(100 * grams)
    ///
    /// Implementation (fixed-point):
    /// - gain_cg_per_count = round(100 * gain_g_per_count)
    /// - offset_cg = round(100 * offset_g)
    /// - result_cg = saturating_mul(gain_cg_per_count, raw - zero_counts) + offset_cg
    ///
    /// Rationale:
    /// - Avoids per-sample floating-point math in the control loop.
    /// - Keeps all controller thresholds and comparisons in one integer unit (cg).
    /// - Uses saturating arithmetic operations (saturating_sub/mul/add) to avoid
    ///   overflow on extreme inputs/parameters.
    ///
    /// Rounding and error bounds:
    /// - `gain_g_per_count` and `offset_g` are rounded to the nearest centigram
    ///   before use. The returned value is typically within ~0.5 cg of
    ///   `round(100 * to_grams(raw))` given stable parameters.
    /// - Non-finite parameters (NaN/±Inf) are treated as 0 during quantization.
    ///
    /// Units:
    /// - Input: `raw` is ADC counts.
    /// - Output: integer centigrams (cg).
    ///
    /// Example:
    /// - If `gain_g_per_count = 0.01`, `zero_counts = 0`, `offset_g = 0.0`,
    ///   and `raw = 123`, then `to_cg(123) == 123` (i.e., 1.23 g).
    pub fn to_cg(&self, raw: i32) -> i32 {
        let delta = raw.saturating_sub(self.zero_counts);
        let gain_cg_per_count = quantize_to_cg_i32(self.gain_g_per_count);
        let offset_cg = quantize_to_cg_i32(self.offset_g);
        gain_cg_per_count
            .saturating_mul(delta)
            .saturating_add(offset_cg)
    }
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            gain_g_per_count: 0.01, // 1 count = 0.01 g (centigram), matches sim
            zero_counts: 0,
            offset_g: 0.0,
        }
    }
}

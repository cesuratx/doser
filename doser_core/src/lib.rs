#![cfg_attr(all(not(debug_assertions), not(test)), deny(warnings))]
#![cfg_attr(
    all(not(debug_assertions), not(test)),
    deny(clippy::all, clippy::pedantic, clippy::nursery)
)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
//! Core dosing logic (hardware-agnostic).
//!
//! This crate provides the hardware-independent dosing engine. All hardware
//! interactions go through `doser_traits::Scale` and `doser_traits::Motor` traits.
//!
//! ## Architecture
//!
//! - **Calibration**: Linear model for raw→grams conversion (`calibration` module)
//! - **Configuration**: All config structs (`config` module)
//! - **Filtering**: Median, moving average, EMA smoothing
//! - **Control**: Multi-speed control with hysteresis (`DoserCore`)
//! - **Safety**: Watchdogs for runtime, overshoot, no-progress
//! - **Status**: Dosing state machine (`status` module)
//!
//! ## Fixed-Point Arithmetic
//!
//! Internals operate in **centigrams** (cg, 1 cg = 0.01 g) using `i32` for deterministic
//! behavior. See `Calibration::to_cg` for conversion and `quantize_to_cg_i32` for rounding.

// Module declarations
pub mod error;
pub mod mocks;
pub mod runner;
pub mod sampler;
pub mod util;

// TODO: Future refactoring - extract these types into dedicated modules:
// - calibration.rs: Calibration struct
// - config.rs: FilterCfg, ControlCfg, SafetyCfg, PredictorCfg, Timeouts
// - status.rs: DosingStatus enum
// This will reduce lib.rs from 1600+ lines to ~800 lines

use crate::error::BuildError;
use crate::error::{AbortReason, DoserError, Result};
use doser_traits::clock::{Clock, MonotonicClock};
use eyre::WrapErr;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

// For typed hardware error mapping
use doser_hardware::error::HwError;

use crate::util::div_round_nearest_i32;

/// Average of two i32 values rounded to nearest with ties away from zero.
/// Uses 64-bit intermediates; cannot overflow and the average fits in i32.
#[inline]
fn avg2_round_nearest_i32(a: i32, b: i32) -> i32 {
    let s = (a as i64) + (b as i64);
    if s >= 0 {
        ((s + 1) / 2) as i32
    } else {
        ((s - 1) / 2) as i32
    }
}

/// Quantize a floating-point grams value to integer centigrams (cg), rounding to nearest
/// and clamping to the i32 range. Non-finite values (NaN/±Inf) map to 0.
#[inline]
fn quantize_to_cg_i32(x_g: f32) -> i32 {
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
/// Notes:
/// - Uses 64-bit intermediates to avoid overflow during subtraction.
/// - For any `i32` inputs, `|a - b| <= u32::MAX` (the maximum occurs for
///   `(i32::MIN, i32::MAX)`), so the final cast to `u32` is always lossless.
#[inline]
fn abs_diff_i32_u32(a: i32, b: i32) -> u32 {
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

#[cfg(test)]
mod abs_diff_tests {
    use super::abs_diff_i32_u32;

    #[test]
    fn handles_extremes_losslessly() {
        let v = abs_diff_i32_u32(i32::MIN, i32::MAX);
        assert_eq!(v, u32::MAX);
    }

    #[test]
    fn simple_pairs() {
        assert_eq!(abs_diff_i32_u32(123, -456), 579);
        assert_eq!(abs_diff_i32_u32(-456, 123), 579);
        assert_eq!(abs_diff_i32_u32(0, 0), 0);
    }
}

#[cfg(test)]
mod avg2_tests {
    use super::avg2_round_nearest_i32;

    #[test]
    fn extremes_and_signs() {
        assert_eq!(avg2_round_nearest_i32(i32::MAX, i32::MAX), i32::MAX);
        assert_eq!(avg2_round_nearest_i32(i32::MIN, i32::MIN), i32::MIN);
        // Cross extremes: sum = -1 -> avg = -0.5 -> away from zero => -1
        assert_eq!(avg2_round_nearest_i32(i32::MAX, i32::MIN), -1);
    }

    #[test]
    fn simple_pairs() {
        assert_eq!(avg2_round_nearest_i32(1, 2), 2); // 1.5 -> 2
        assert_eq!(avg2_round_nearest_i32(-1, 0), -1); // -0.5 -> -1
        assert_eq!(avg2_round_nearest_i32(10, 10), 10);
        assert_eq!(avg2_round_nearest_i32(-5, -6), -6); // -5.5 -> -6
    }
}

/// Simple linear calibration from raw scale counts to grams.
/// grams = gain_g_per_count * (raw - zero_counts) + offset_g
#[derive(Debug, Clone)]
pub struct Calibration {
    pub gain_g_per_count: f32,
    pub zero_counts: i32,
    pub offset_g: f32,
}
impl Calibration {
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

/// Public status of a single step of the dosing loop.
#[derive(Debug)]
pub enum DosingStatus {
    /// Keep going; not settled yet.
    Running,
    /// Target reached and settled; motor already stopped.
    Complete,
    /// Aborted with a typed error; motor has been asked to stop.
    Aborted(DoserError),
}

/// Filter configuration (keep it simple for now).
#[derive(Debug, Clone)]
pub struct FilterCfg {
    /// moving average window size (not used in this minimal variant)
    pub ma_window: usize,
    /// median prefilter window size (not used here)
    pub median_window: usize,
    /// sampling rate in Hz (informational)
    pub sample_rate_hz: u32,
    /// Optional EMA smoothing factor; when > 0, EMA is used instead of moving average.
    /// Range: (0.0, 1.0]. 0.0 disables EMA and uses moving average when ma_window > 1.
    pub ema_alpha: f32,
}

/// Filter selection for the smoothing stage (after optional median).
/// This is informational; the active variant is derived from `FilterCfg`.
#[derive(Debug, Clone, Copy)]
pub enum FilterKind {
    MovingAverage { window: usize },
    Median { window: usize },
    Ema { alpha: f32 }, // 0<alpha<=1
}

impl Default for FilterCfg {
    fn default() -> Self {
        Self {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
            ema_alpha: 0.0,
        }
    }
}

/// Control configuration.
#[derive(Debug, Clone)]
pub struct ControlCfg {
    /// Optional speed table: each entry is (threshold_g, sps). Precedence over two-speed mode.
    /// When non-empty, sorted descending by threshold during build.
    pub speed_bands: Vec<(f32, u32)>,
    /// Switch to fine speed once err <= slow_at_g (used when speed_bands.is_empty())
    pub slow_at_g: f32,
    /// Consider “in band” if |err| <= hysteresis_g. Default: 0.07 g.
    pub hysteresis_g: f32,
    /// Reported stable if weight stays within hysteresis for this many ms
    pub stable_ms: u64,
    /// Coarse motor speed (steps per second or implementation-defined)
    pub coarse_speed: u32,
    /// Fine motor speed for the final approach
    pub fine_speed: u32,
    /// Additional tolerance below target (grams) to consider completion zone. Default: 0.08 g.
    pub epsilon_g: f32,
}

impl Default for ControlCfg {
    fn default() -> Self {
        Self {
            // Default table (descending thresholds in build)
            speed_bands: vec![(1.0, 1100), (0.5, 450), (0.2, 200)],
            slow_at_g: 1.0,
            hysteresis_g: 0.07,
            stable_ms: 250,
            coarse_speed: 1200,
            fine_speed: 250,
            epsilon_g: 0.08,
        }
    }
}

/// Predictor configuration for early motor stop to reduce overshoot.
///
/// Disabled by default to preserve existing behavior unless explicitly enabled.
#[derive(Debug, Clone)]
pub struct PredictorCfg {
    /// Enable the predictor logic.
    pub enabled: bool,
    /// Rolling window size for the slope estimate (in samples).
    pub window: usize,
    /// Extra latency margin (ms) to account for sensor/control/filter lag.
    pub extra_latency_ms: u64,
    /// Minimum fraction of target progress before predictor activates (0.0..=1.0).
    pub min_progress_ratio: f32,
}

impl Default for PredictorCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            window: 6,
            extra_latency_ms: 20,
            min_progress_ratio: 0.10,
        }
    }
}

/// Safety configuration for runtime and overshoot guards.
#[derive(Debug, Clone)]
pub struct SafetyCfg {
    /// Hard cap on a single dosing run runtime in milliseconds.
    pub max_run_ms: u64,
    /// Abort if weight exceeds target by more than this many grams.
    pub max_overshoot_g: f32,
    /// Abort if weight doesn't change by at least this many grams
    /// for at least `no_progress_ms` while motor is commanded to run.
    /// Set to 0 to disable.
    pub no_progress_epsilon_g: f32,
    /// See `no_progress_epsilon_g`. 0 disables the watchdog.
    pub no_progress_ms: u64,
}

impl Default for SafetyCfg {
    fn default() -> Self {
        Self {
            max_run_ms: 60_000,   // 60s
            max_overshoot_g: 2.0, // 2g
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        }
    }
}

/// Timeouts and watchdogs.
#[derive(Debug, Clone)]
pub struct Timeouts {
    /// Max sensor wait per read (ms)
    pub sensor_ms: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { sensor_ms: 150 }
    }
}

/// Unified core for both dynamic (boxed) and generic (static dispatch) variants.
pub struct DoserCore<S: doser_traits::Scale, M: doser_traits::Motor> {
    scale: S,
    motor: M,
    filter: FilterCfg,
    control: ControlCfg,
    safety: SafetyCfg,
    timeouts: Timeouts,
    calibration: Calibration,
    // Target in centigrams (fixed-point: 1 = 0.01 g)
    target_cg: i32,
    // Unified clock for deterministic time in tests
    pub(crate) clock: Arc<dyn Clock + Send + Sync>,
    // Epoch Instant for computing monotonic milliseconds
    epoch: Instant,

    // Last observed weight in centigrams
    last_weight_cg: i32,
    settled_since_ms: Option<u64>,
    // Track when the dosing run started to enforce a max runtime (ms since epoch at begin)
    start_ms: u64,
    // Buffers for filtering (centigrams)
    ma_buf: VecDeque<i32>,
    med_buf: VecDeque<i32>,
    // EMA state in centigrams (floating for fractional smoothing); None until first sample
    ema_prev_cg: Option<f32>,
    // Temporary preallocated buffer to compute medians without per-step allocation
    tmp_med_buf: Vec<i32>,
    // Cached control-loop sleep period in microseconds to avoid repeated division
    period_us: u64,
    // Cached quantized calibration parameters (centigrams per count and offset)
    cal_gain_cg_per_count: i32,
    cal_offset_cg: i32,
    // Cached integer thresholds (centigrams)
    slow_at_cg: i32,
    epsilon_cg: i32,
    max_overshoot_cg: i32,
    no_progress_epsilon_cg: i32,
    // Motor lifecycle
    motor_started: bool,
    // Optional E-stop callback; if returns true, abort immediately (after debounce)
    estop_check: Option<Box<dyn Fn() -> bool>>,
    // No-progress watchdog uses centigrams
    last_progress_cg: i32,
    last_progress_at_ms: u64,
    // Latch E-stop condition until begin() is called again
    estop_latched: bool,
    // Debounce config and counter
    estop_debounce_n: u8,
    estop_count: u8,

    // Stop-early predictor state
    predictor: PredictorCfg,
    pred_hist: VecDeque<(u64, i32)>, // (ms_since_epoch, weight_cg)
    pred_latency_ms: u64,            // effective latency budget (ms)

    // Telemetry for CLI JSON and debugging
    last_slope_ema_cg_per_ms: Option<f32>,
    last_inflight_cg: Option<i32>,
    early_stop_at_cg: Option<i32>,

    // Cached speed bands (thresholds in centigrams), sorted descending by threshold
    speed_bands_cg: Vec<(i32, u32)>,
}

impl<S: doser_traits::Scale, M: doser_traits::Motor> core::fmt::Debug for DoserCore<S, M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DoserCore")
            .field("target_g", &((self.target_cg as f32) / 100.0))
            .field("last_weight_g", &((self.last_weight_cg as f32) / 100.0))
            .field("motor_started", &self.motor_started)
            .finish()
    }
}

impl<S: doser_traits::Scale, M: doser_traits::Motor> DoserCore<S, M> {
    /// Return the last observed weight in grams.
    pub fn last_weight(&self) -> f32 {
        (self.last_weight_cg as f32) / 100.0
    }

    /// Optionally set the tare baseline in raw counts.
    pub fn set_tare_counts(&mut self, zero_counts: i32) {
        self.calibration.zero_counts = zero_counts;
    }

    /// Return the configured filter parameters (currently informational).
    pub fn filter_cfg(&self) -> &FilterCfg {
        &self.filter
    }

    /// Process a pre-sampled raw reading (centers Phase 3 sampler integration).
    pub fn step_from_raw(&mut self, raw: i32) -> Result<DosingStatus> {
        if self.estop_latched || self.poll_estop() {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on estop");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(AbortReason::Estop)));
        }
        let w_cg_raw = self.to_cg_cached(raw);
        let w_cg = self.apply_filter(w_cg_raw);
        self.last_weight_cg = w_cg;
        let err_cg = self.target_cg - w_cg;
        // unsigned_abs() returns u32, which safely represents |i32::MIN| without overflow.
        // Equivalent to `err_cg.abs() as u32`, but without the potential panic on i32::MIN.
        let abs_err_cg = err_cg.unsigned_abs();
        let now = self.clock.ms_since(self.epoch);
        if now.saturating_sub(self.start_ms) >= self.safety.max_run_ms {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on max-run cap");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::MaxRuntime,
            )));
        }
        if w_cg > self.target_cg + self.max_overshoot_cg {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on overshoot");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::Overshoot,
            )));
        }
        // Optional early-stop predictor to reduce overshoot under latency.
        if self.maybe_early_stop(now, w_cg) {
            // Throttle loop after issuing early stop
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        }

        if w_cg + self.epsilon_cg >= self.target_cg {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed entering settle zone");
            }
            // Start settling window unconditionally once in completion zone.
            if self.settled_since_ms.is_none() {
                self.settled_since_ms = Some(now);
            }
            if let Some(since) = self.settled_since_ms
                && now.saturating_sub(since) >= self.control.stable_ms
            {
                return Ok(DosingStatus::Complete);
            }
            // Keep polling while settled window accrues.
            // Throttle loop to the configured sample rate.
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        } else {
            // Below target: not in completion zone; reset settle timer
            self.settled_since_ms = None;
        }
        // Speed selection: speed_bands take precedence when non-empty
        let mut selected_band: Option<(i32, u32)> = None;
        let mut target_speed = self.control.coarse_speed;
        if !self.speed_bands_cg.is_empty() {
            let err_g = (err_cg.max(0) as f32) / 100.0;
            for (thr_cg, sps) in &self.speed_bands_cg {
                if err_cg >= *thr_cg {
                    selected_band = Some((*thr_cg, *sps));
                    target_speed = *sps;
                    break;
                }
            }
            if selected_band.is_none()
                && let Some((thr_cg, sps)) = self.speed_bands_cg.last().copied()
            {
                selected_band = Some((thr_cg, sps));
                target_speed = sps;
            }
            let thr_g = selected_band
                .map(|(cg, _)| (cg as f32) / 100.0)
                .unwrap_or(0.0);
            let sps = target_speed;
            tracing::trace!(
                err_g,
                band_threshold_g = thr_g,
                band_sps = sps,
                "speed band select"
            );
        } else {
            // fallback to legacy 2-speed proportional taper
            if self.slow_at_cg > 0 && abs_err_cg <= self.slow_at_cg as u32 {
                let ratio = (abs_err_cg as f32 / self.slow_at_cg as f32).clamp(0.0, 1.0);
                let min_frac = 0.2_f32;
                let frac = min_frac + (1.0 - min_frac) * ratio;
                target_speed = ((self.control.fine_speed as f32 * frac).max(1.0)) as u32;
            } else {
                target_speed = self.control.coarse_speed;
            }
            tracing::trace!(
                err_g = (err_cg.max(0) as f32) / 100.0,
                band_threshold_g = 0.0,
                band_sps = target_speed,
                "speed band select (legacy)"
            );
        }
        if self.safety.no_progress_ms > 0 && self.no_progress_epsilon_cg > 0 && target_speed > 0 {
            let progress_delta_cg = abs_diff_i32_u32(w_cg, self.last_progress_cg);
            if progress_delta_cg >= self.no_progress_epsilon_cg as u32 {
                self.last_progress_cg = w_cg;
                self.last_progress_at_ms = now;
            } else if now.saturating_sub(self.last_progress_at_ms) >= self.safety.no_progress_ms {
                if let Err(e) = self.motor_stop() {
                    tracing::warn!(error = %e, "motor_stop failed on no-progress watchdog");
                }
                return Ok(DosingStatus::Aborted(DoserError::Abort(
                    AbortReason::NoProgress,
                )));
            }
        }
        if !self.motor_started {
            self.motor
                .start()
                .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
                .wrap_err("motor start")?;
            self.motor_started = true;
        }
        self.motor
            .set_speed(target_speed)
            .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
            .wrap_err("set_speed")?;
        self.clock.sleep(Duration::from_micros(self.period_us));
        Ok(DosingStatus::Running)
    }

    #[inline]
    fn to_cg_cached(&self, raw: i32) -> i32 {
        let delta = raw.saturating_sub(self.calibration.zero_counts);
        self.cal_gain_cg_per_count
            .saturating_mul(delta)
            .saturating_add(self.cal_offset_cg)
    }

    /// Reset per-run state (start time, settling window). Call before a new dose.
    pub fn begin(&mut self) {
        // Reset epoch; subsequent ms are measured from here
        self.epoch = self.clock.now();
        let now = self.clock.ms_since(self.epoch); // will be 0
        self.start_ms = now;
        self.settled_since_ms = None;
        // Clear filter state for a fresh run
        self.ma_buf.clear();
        self.med_buf.clear();
        self.ema_prev_cg = None;
        self.last_weight_cg = 0;
        self.motor_started = false;
        self.last_progress_cg = 0;
        self.last_progress_at_ms = now;
        self.estop_latched = false;
        self.estop_count = 0;
        // Reset predictor history
        self.pred_hist.clear();
        // Reset telemetry
        self.last_slope_ema_cg_per_ms = None;
        self.last_inflight_cg = None;
        self.early_stop_at_cg = None;
    }

    /// Stop the motor (best-effort).
    pub fn motor_stop(&mut self) -> Result<()> {
        self.motor
            .stop()
            .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
            .wrap_err("motor_stop")
    }

    /// Telemetry: last slope EMA in grams per second (approx), if available.
    pub fn last_slope_ema_gps(&self) -> Option<f32> {
        self.last_slope_ema_cg_per_ms.map(|v| v * 0.01 * 1000.0)
    }
    /// Telemetry: inflight mass estimate in grams at last check, if available.
    pub fn last_inflight_g(&self) -> Option<f32> {
        self.last_inflight_cg.map(|cg| (cg as f32) * 0.01)
    }
    /// Telemetry: weight at which predictor triggered early stop, in grams, if any.
    pub fn early_stop_at_g(&self) -> Option<f32> {
        self.early_stop_at_cg.map(|cg| (cg as f32) * 0.01)
    }

    /// Poll the E-stop input with debounce; returns true if latched.
    fn poll_estop(&mut self) -> bool {
        if let Some(check) = &self.estop_check {
            if check() {
                self.estop_count = self.estop_count.saturating_add(1);
                if self.estop_count >= self.estop_debounce_n {
                    self.estop_latched = true;
                }
            } else {
                self.estop_count = 0;
            }
        }
        self.estop_latched
    }
    fn apply_filter(&mut self, w_cg: i32) -> i32 {
        // Ensure sane window sizes
        let med_win = self.filter.median_window.max(1);
        let ma_win = self.filter.ma_window.max(1);
        let ema_alpha = if self.filter.ema_alpha.is_finite() {
            self.filter.ema_alpha
        } else {
            0.0
        };

        // Median prefilter over centigrams
        let after_median = if med_win > 1 {
            self.med_buf.push_back(w_cg);
            if self.med_buf.len() > med_win {
                self.med_buf.pop_front();
            }
            // Reuse a preallocated buffer to avoid per-step allocations
            self.tmp_med_buf.clear();
            self.tmp_med_buf.extend(self.med_buf.iter().copied());
            self.tmp_med_buf.sort_unstable();
            let n = self.tmp_med_buf.len();
            // Invariants (only applicable when med_win > 1, i.e., when this branch executes):
            // - We just pushed `w_cg` into `med_buf`, so `med_buf.len() >= 1`.
            // - After the optional pop, `med_buf.len() <= med_win`.
            // - `tmp_med_buf` is a copy of `med_buf`, so lengths match and `n >= 1`.
            // The debug assertions below validate these invariants to guard the safe
            // indexing that follows (accessing `mid` and `mid-1` on even `n`). If future
            // changes break the push/pop discipline or window sizing, these will trip in
            // debug builds and surface the logic error early.
            #[cfg(debug_assertions)]
            {
                let med_len = self.med_buf.len();
                debug_assert!(n > 0, "median buffer unexpectedly empty");
                debug_assert_eq!(med_len, n);
                debug_assert!(n <= med_win, "median buffer exceeded window size");
            }
            let mid = n / 2;
            if n.is_multiple_of(2) {
                // n >= 2 here, so mid >= 1 and mid-1 is safe
                let a = self.tmp_med_buf[mid - 1];
                let b = self.tmp_med_buf[mid];
                // Use a specialized avg(2) helper to avoid any panic path and keep ties away from zero.
                avg2_round_nearest_i32(a, b)
            } else {
                self.tmp_med_buf[mid]
            }
        } else {
            w_cg
        };

        // Smoothing stage: EMA (when enabled) else Moving Average, else passthrough
        if ema_alpha > 0.0 {
            // Initialize EMA with first value to avoid startup bias
            let x = after_median as f32;
            let alpha = ema_alpha.clamp(0.0, 1.0);
            let y = match self.ema_prev_cg {
                None => x,
                Some(prev) => alpha * x + (1.0 - alpha) * prev,
            };
            self.ema_prev_cg = Some(y);
            // Round to nearest centigram
            y.round() as i32
        } else if ma_win > 1 {
            self.ma_buf.push_back(after_median);
            if self.ma_buf.len() > ma_win {
                self.ma_buf.pop_front();
            }
            // Sum in i128 to avoid overflow for any realistic window size. Even at extreme
            // values, the average of i32 samples is guaranteed to fit in i32.
            let sum_i128: i128 = self.ma_buf.iter().map(|&v| v as i128).sum();
            let len_i32 = self.ma_buf.len() as i32;
            // Avoid truncating before range checks; prefer the fast i32 path when safe.
            if len_i32 > 0 {
                if (i32::MIN as i128..=i32::MAX as i128).contains(&sum_i128) {
                    div_round_nearest_i32(sum_i128 as i32, len_i32)
                } else {
                    let n = len_i32 as i128;
                    let q = if sum_i128 >= 0 {
                        (sum_i128 + n / 2) / n
                    } else {
                        (sum_i128 - n / 2) / n
                    };
                    #[cfg(debug_assertions)]
                    debug_assert!(
                        (i32::MIN as i128..=i32::MAX as i128).contains(&q),
                        "moving-average result out of i32 range"
                    );
                    q as i32
                }
            } else {
                0
            }
        } else {
            after_median
        }
    }

    /// One iteration of the dosing loop
    pub fn step(&mut self) -> Result<DosingStatus> {
        // If previously estopped, keep aborting until reset via begin()
        if self.estop_latched || self.poll_estop() {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on estop");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(AbortReason::Estop)));
        }

        // 1) read current weight
        let timeout = Duration::from_millis(self.timeouts.sensor_ms);
        let raw = self
            .scale
            .read(timeout)
            .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
            .wrap_err("reading scale")?;

        // Apply calibration (raw counts -> centigrams) via cached integer path and filtering
        let w_cg_raw = self.to_cg_cached(raw);
        let w_cg = self.apply_filter(w_cg_raw);

        self.last_weight_cg = w_cg;
        let err_cg = self.target_cg - w_cg;
        let abs_err_cg = err_cg.unsigned_abs();

        let now = self.clock.ms_since(self.epoch);

        // 1a) hard runtime cap (>= to allow deterministic tests with 0ms)
        if now.saturating_sub(self.start_ms) >= self.safety.max_run_ms {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on max-run cap");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::MaxRuntime,
            )));
        }

        // 1b) excessive overshoot guard
        if w_cg > self.target_cg + self.max_overshoot_cg {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on overshoot");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::Overshoot,
            )));
        }

        // Optional early-stop predictor to reduce overshoot under latency.
        if self.maybe_early_stop(now, w_cg) {
            // Throttle loop after issuing early stop
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        }

        // 2) Reached or exceeded target? Stop and settle (asymmetric completion)
        if w_cg + self.epsilon_cg >= self.target_cg {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed entering settle zone");
            }
            // Start settling window unconditionally once in completion zone.
            if self.settled_since_ms.is_none() {
                self.settled_since_ms = Some(now);
            }
            if let Some(since) = self.settled_since_ms
                && now.saturating_sub(since) >= self.control.stable_ms
            {
                return Ok(DosingStatus::Complete);
            }
            // Keep polling while settled window accrues.
            // Throttle loop to the configured sample rate.
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        } else {
            // Below target: not in completion zone; reset settle timer
            self.settled_since_ms = None;
        }

        // 3) choose speed via bands or legacy fallback
        let mut selected_band: Option<(i32, u32)> = None;
        let mut target_speed = self.control.coarse_speed;
        if !self.speed_bands_cg.is_empty() {
            let err_g = (err_cg.max(0) as f32) / 100.0;
            for (thr_cg, sps) in &self.speed_bands_cg {
                if err_cg >= *thr_cg {
                    selected_band = Some((*thr_cg, *sps));
                    target_speed = *sps;
                    break;
                }
            }
            if selected_band.is_none()
                && let Some((thr_cg, sps)) = self.speed_bands_cg.last().copied()
            {
                selected_band = Some((thr_cg, sps));
                target_speed = sps;
            }
            let thr_g = selected_band
                .map(|(cg, _)| (cg as f32) / 100.0)
                .unwrap_or(0.0);
            let sps = target_speed;
            tracing::trace!(
                err_g,
                band_threshold_g = thr_g,
                band_sps = sps,
                "speed band select"
            );
        } else {
            if self.slow_at_cg > 0 && abs_err_cg <= self.slow_at_cg as u32 {
                let ratio = (abs_err_cg as f32 / self.slow_at_cg as f32).clamp(0.0, 1.0);
                let min_frac = 0.2_f32; // floor at 20% of fine speed
                let frac = min_frac + (1.0 - min_frac) * ratio; // [min_frac, 1.0]
                target_speed = ((self.control.fine_speed as f32 * frac).max(1.0)) as u32;
            } else {
                target_speed = self.control.coarse_speed;
            }
            tracing::trace!(
                err_g = (err_cg.max(0) as f32) / 100.0,
                band_threshold_g = 0.0,
                band_sps = target_speed,
                "speed band select (legacy)"
            );
        }

        // 3a) no-progress watchdog (only while motor is commanded to run)
        if self.safety.no_progress_ms > 0 && self.no_progress_epsilon_cg > 0 && target_speed > 0 {
            let progress_delta_cg = abs_diff_i32_u32(w_cg, self.last_progress_cg);
            if progress_delta_cg >= self.no_progress_epsilon_cg as u32 {
                self.last_progress_cg = w_cg;
                self.last_progress_at_ms = now;
            } else if now.saturating_sub(self.last_progress_at_ms) >= self.safety.no_progress_ms {
                if let Err(e) = self.motor_stop() {
                    tracing::warn!(error = %e, "motor_stop failed on no-progress watchdog");
                }
                return Ok(DosingStatus::Aborted(DoserError::Abort(
                    AbortReason::NoProgress,
                )));
            }
        }

        // Ensure motor is started before commanding speed
        if !self.motor_started {
            self.motor
                .start()
                .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
                .wrap_err("motor start")?;
            self.motor_started = true;
        }

        self.motor
            .set_speed(target_speed)
            .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
            .wrap_err("set_speed")?;

        // Throttle loop to the configured sample rate.
        self.clock.sleep(Duration::from_micros(self.period_us));

        Ok(DosingStatus::Running)
    }
}

impl<S: doser_traits::Scale, M: doser_traits::Motor> DoserCore<S, M> {
    /// Update predictor history and decide whether to stop early this iteration.
    /// Returns true if an early stop was issued.
    #[inline]
    fn maybe_early_stop(&mut self, now_ms: u64, w_cg: i32) -> bool {
        if !self.predictor.enabled {
            return false;
        }
        // Gate on minimum progress ratio to avoid triggering too early/noisy start.
        if self.target_cg > 0 {
            let progress = (w_cg as f32) / (self.target_cg as f32);
            if progress < self.predictor.min_progress_ratio {
                // Still update history for future iterations
                self.pred_hist.push_back((now_ms, w_cg));
                if self.pred_hist.len() > self.predictor.window.max(1) {
                    self.pred_hist.pop_front();
                }
                return false;
            }
        }

        // Maintain rolling window
        self.pred_hist.push_back((now_ms, w_cg));
        let max_len = self.predictor.window.max(1);
        if self.pred_hist.len() > max_len {
            self.pred_hist.pop_front();
        }
        if self.pred_hist.len() < 2 {
            return false;
        }
        // Use simple slope estimate: (last - first) / dt
        let Some((t0, w0)) = self.pred_hist.front().copied() else {
            return false;
        };
        let dt_ms = now_ms.saturating_sub(t0);
        if dt_ms == 0 {
            return false;
        }
        let dw_cg = (w_cg as i64) - (w0 as i64);
        if dw_cg <= 0 {
            return false; // no upward trend
        }

        // inflight_cg = round(dw_cg * pred_latency_ms / dt_ms)
        // Use i64 with saturating_mul; preserve sign-aware rounding semantics.
        let num: i64 = dw_cg.saturating_mul(self.pred_latency_ms as i64);
        let den: i64 = (dt_ms as i64).max(1);
        let half = den >> 1;
        let inflight_i64 = if num >= 0 {
            (num + half) / den
        } else {
            (num - half) / den
        };
        let inflight_cg = inflight_i64.clamp(i32::MIN as i64, i32::MAX as i64) as i32;

        // Update telemetry: slope EMA (cg/ms) and inflight mass (cg)
        let slope_cg_per_ms = (dw_cg as f32) / (den as f32); // den=dt_ms
        let alpha = if self.filter.ema_alpha.is_finite() && self.filter.ema_alpha > 0.0 {
            self.filter.ema_alpha
        } else {
            0.3
        };
        self.last_slope_ema_cg_per_ms = Some(match self.last_slope_ema_cg_per_ms {
            None => slope_cg_per_ms,
            Some(prev) => alpha * slope_cg_per_ms + (1.0 - alpha) * prev,
        });
        self.last_inflight_cg = Some(inflight_cg);

        let predicted = w_cg
            .saturating_add(inflight_cg)
            .saturating_add(self.epsilon_cg);
        if predicted >= self.target_cg {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on predictor early-stop");
            }
            // Record early stop point (cg)
            self.early_stop_at_cg = Some(w_cg);
            tracing::debug!(
                w_cg,
                inflight_cg,
                dt_ms,
                window = self.pred_hist.len(),
                "predictor early-stop issued"
            );
            return true;
        }
        false
    }
}

/// Public dynamic (boxed) doser that preserves the existing API via composition.
pub struct Doser {
    inner: DoserCore<Box<dyn doser_traits::Scale>, Box<dyn doser_traits::Motor>>,
}

impl core::fmt::Debug for Doser {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Doser")
            .field("target_g", &((self.inner.target_cg as f32) / 100.0))
            .field(
                "last_weight_g",
                &((self.inner.last_weight_cg as f32) / 100.0),
            )
            .field("motor_started", &self.inner.motor_started)
            .finish()
    }
}

impl Doser {
    /// Start building a Doser.
    pub fn builder() -> DoserBuilder<Missing, Missing, Missing> {
        DoserBuilder::default()
    }

    /// Return the last observed weight in grams.
    pub fn last_weight(&self) -> f32 {
        self.inner.last_weight()
    }

    /// Optionally set the tare baseline in raw counts.
    pub fn set_tare_counts(&mut self, zero_counts: i32) {
        self.inner.set_tare_counts(zero_counts);
    }

    /// Return the configured filter parameters (currently informational).
    pub fn filter_cfg(&self) -> &FilterCfg {
        self.inner.filter_cfg()
    }

    /// Process a pre-sampled raw reading (centers Phase 3 sampler integration).
    pub fn step_from_raw(&mut self, raw: i32) -> Result<DosingStatus> {
        self.inner.step_from_raw(raw)
    }

    /// Reset per-run state (start time, settling window). Call before a new dose.
    pub fn begin(&mut self) {
        self.inner.begin();
    }

    /// One iteration of the dosing loop
    pub fn step(&mut self) -> Result<DosingStatus> {
        self.inner.step()
    }

    /// Stop the motor (best-effort).
    pub fn motor_stop(&mut self) -> Result<()> {
        self.inner.motor_stop()
    }

    /// Telemetry: last slope EMA in grams per second (approx), if available.
    pub fn last_slope_ema_gps(&self) -> Option<f32> {
        self.inner
            .last_slope_ema_cg_per_ms
            .map(|v| v * 0.01 * 1000.0)
    }
    /// Telemetry: inflight mass estimate in grams at last check, if available.
    pub fn last_inflight_g(&self) -> Option<f32> {
        self.inner.last_inflight_cg.map(|cg| (cg as f32) * 0.01)
    }
    /// Telemetry: weight at which predictor triggered early stop, in grams, if any.
    pub fn early_stop_at_g(&self) -> Option<f32> {
        self.inner.early_stop_at_cg.map(|cg| (cg as f32) * 0.01)
    }
}

// Map any error to a typed DoserError, with special handling for hardware errors.
fn map_hw_error_dyn(e: &(dyn std::error::Error + 'static)) -> DoserError {
    if let Some(hw) = e.downcast_ref::<HwError>() {
        match hw {
            HwError::Timeout => DoserError::Timeout,
            HwError::DataReadyTimeout => DoserError::Timeout,
            other => DoserError::HardwareFault(other.to_string()),
        }
    } else {
        let s = e.to_string();
        if s.to_lowercase().contains("timeout") {
            DoserError::Timeout
        } else {
            DoserError::Hardware(s)
        }
    }
}

// Type-state markers for the builder
pub struct Missing;
pub struct Set;

use std::marker::PhantomData;

/// Builder for `Doser`. All fields are validated on `build()`.
#[derive(Default)]
pub struct DoserBuilder<S, M, T> {
    scale: Option<Box<dyn doser_traits::Scale>>,
    motor: Option<Box<dyn doser_traits::Motor>>,
    filter: Option<FilterCfg>,
    control: Option<ControlCfg>,
    safety: Option<SafetyCfg>,
    timeouts: Option<Timeouts>,
    calibration: Option<Calibration>,
    target_g: Option<f32>,
    _calibration_loaded: bool,
    // Optional E-stop
    estop_check: Option<Box<dyn Fn() -> bool>>,
    // Optional clock for tests (accept Box here)
    clock: Option<Box<dyn Clock + Send + Sync>>,
    // E-stop debounce configuration
    estop_debounce_n: Option<u8>,
    // Optional predictor configuration
    predictor: Option<PredictorCfg>,
    // Type-state markers
    _s: PhantomData<S>,
    _m: PhantomData<M>,
    _t: PhantomData<T>,
}

impl Default for DoserBuilder<Missing, Missing, Missing> {
    fn default() -> Self {
        Self {
            scale: None,
            motor: None,
            filter: None,
            control: None,
            safety: None,
            timeouts: None,
            calibration: None,
            target_g: None,
            _calibration_loaded: false,
            estop_check: None,
            clock: None,
            estop_debounce_n: None,
            predictor: None,
            _s: PhantomData,
            _m: PhantomData,
            _t: PhantomData,
        }
    }
}

impl<S, M, T> DoserBuilder<S, M, T> {
    /// Fallible build available in any type-state; returns detailed BuildError for missing pieces.
    pub fn try_build(self) -> Result<Doser> {
        let DoserBuilder {
            scale,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            _calibration_loaded: _,
            estop_check,
            clock,
            estop_debounce_n,
            predictor,
            _s: _,
            _m: _,
            _t: _,
        } = self;

        let scale = scale.ok_or_else(|| eyre::Report::new(BuildError::MissingScale))?;
        let motor = motor.ok_or_else(|| eyre::Report::new(BuildError::MissingMotor))?;
        let target_g = target_g.ok_or_else(|| eyre::Report::new(BuildError::MissingTarget))?;

        if !(0.1..=5000.0).contains(&target_g) {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "target grams out of range",
            )));
        }

        let filter = filter.unwrap_or_default();
        let mut control = control.unwrap_or_default();
        let predictor = predictor.unwrap_or_default();
        let safety = safety.unwrap_or_default();
        let timeouts = timeouts.unwrap_or_default();
        let calibration = calibration.unwrap_or_default();
        let clock: Arc<dyn Clock + Send + Sync> = match clock {
            Some(b) => Arc::from(b),
            None => Arc::new(MonotonicClock::new()),
        };
        let estop_debounce_n = estop_debounce_n.unwrap_or(2);

        // Validate configs (non-panicking; return typed Config errors)
        if control.hysteresis_g.is_sign_negative() {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "hysteresis_g must be >= 0",
            )));
        }
        if control.slow_at_g.is_sign_negative() {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "slow_at_g must be >= 0",
            )));
        }
        if control.coarse_speed == 0 || control.fine_speed == 0 {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "motor speeds must be > 0",
            )));
        }
        if timeouts.sensor_ms == 0 {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "sensor_ms must be >= 1",
            )));
        }
        if safety.max_overshoot_g.is_sign_negative() {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "max_overshoot_g must be >= 0",
            )));
        }
        if safety.no_progress_epsilon_g.is_sign_negative() {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "no_progress_epsilon_g must be > 0",
            )));
        }
        if filter.sample_rate_hz == 0 {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "sample_rate_hz must be > 0",
            )));
        }
        // Validate speed band entries (if provided)
        if !control.speed_bands.is_empty() {
            for (thr_g, sps) in &control.speed_bands {
                if !thr_g.is_finite() {
                    return Err(eyre::Report::new(BuildError::InvalidConfig(
                        "speed band threshold must be finite",
                    )));
                }
                if *thr_g < 0.0 {
                    return Err(eyre::Report::new(BuildError::InvalidConfig(
                        "speed band threshold must be >= 0",
                    )));
                }
                if *sps == 0 {
                    return Err(eyre::Report::new(BuildError::InvalidConfig(
                        "speed band speed must be > 0",
                    )));
                }
            }
        }

        // Capture capacities before moving filter
        let ma_cap = filter.ma_window.max(1);
        let med_cap = filter.median_window.max(1);

        // Establish epoch for monotonic timing
        let epoch = clock.now();
        let now = clock.ms_since(epoch); // 0

        // Precompute loop period (us)
        let period_us = crate::util::period_us(filter.sample_rate_hz);
        let period_ms = period_us.div_ceil(1000);
        let pred_latency_ms = period_ms.saturating_add(predictor.extra_latency_ms);

        // Sort speed bands descending by threshold and precompute integer thresholds in centigrams
        if !control.speed_bands.is_empty() {
            control.speed_bands.sort_by(|a, b| b.0.total_cmp(&a.0));
        }
        // Precompute integer thresholds in centigrams
        let to_cg = |g: f32| ((g * 100.0).round()) as i32;
        let target_cg = to_cg(target_g);
        let epsilon_cg = to_cg(control.epsilon_g);
        let max_overshoot_cg = to_cg(safety.max_overshoot_g);
        let no_progress_epsilon_cg = to_cg(safety.no_progress_epsilon_g);
        let slow_at_cg = to_cg(control.slow_at_g);
        let speed_bands_cg: Vec<(i32, u32)> = control
            .speed_bands
            .iter()
            .map(|(g, sps)| (to_cg(*g), *sps))
            .collect();

        // Precompute quantized calibration before moving `calibration` into the struct
        let cal_gain_cg_per_count = quantize_to_cg_i32(calibration.gain_g_per_count);
        let cal_offset_cg = quantize_to_cg_i32(calibration.offset_g);

        Ok(Doser {
            inner: DoserCore {
                scale,
                motor,
                filter,
                control,
                safety,
                timeouts,
                calibration,
                target_cg,
                clock,
                epoch,
                last_weight_cg: 0,
                settled_since_ms: None,
                start_ms: now,
                ma_buf: VecDeque::with_capacity(ma_cap),
                med_buf: VecDeque::with_capacity(med_cap),
                tmp_med_buf: Vec::with_capacity(med_cap),
                ema_prev_cg: None,
                period_us,
                cal_gain_cg_per_count,
                cal_offset_cg,
                slow_at_cg,
                epsilon_cg,
                max_overshoot_cg,
                no_progress_epsilon_cg,
                motor_started: false,
                estop_check,
                last_progress_cg: 0,
                last_progress_at_ms: now,
                estop_latched: false,
                estop_debounce_n,
                estop_count: 0,
                predictor,
                pred_hist: VecDeque::with_capacity(8),
                pred_latency_ms,
                speed_bands_cg,
                last_slope_ema_cg_per_ms: None,
                last_inflight_cg: None,
                early_stop_at_cg: None,
            },
        })
    }
}

/// Chainable setters that do not affect type-state
impl<S, M, T> DoserBuilder<S, M, T> {
    pub fn with_filter(mut self, filter: FilterCfg) -> Self {
        self.filter = Some(filter);
        self
    }
    pub fn with_control(mut self, control: ControlCfg) -> Self {
        self.control = Some(control);
        self
    }
    pub fn with_safety(mut self, safety: SafetyCfg) -> Self {
        self.safety = Some(safety);
        self
    }
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = Some(timeouts);
        self
    }
    pub fn with_calibration(mut self, calibration: Calibration) -> Self {
        self.calibration = Some(calibration);
        self._calibration_loaded = true;
        self
    }
    pub fn with_tare_counts(mut self, zero_counts: i32) -> Self {
        let mut c = self.calibration.unwrap_or_default();
        c.zero_counts = zero_counts;
        self.calibration = Some(c);
        self
    }
    pub fn with_calibration_gain_offset(mut self, gain_g_per_count: f32, offset_g: f32) -> Self {
        let mut c = self.calibration.unwrap_or_default();
        c.gain_g_per_count = gain_g_per_count;
        c.offset_g = offset_g;
        self.calibration = Some(c);
        self._calibration_loaded = true;
        self
    }
    pub fn with_estop_check<F>(mut self, f: F) -> Self
    where
        F: Fn() -> bool + 'static,
    {
        self.estop_check = Some(Box::new(f));
        self
    }
    pub fn with_estop_debounce(mut self, n: u8) -> Self {
        self.estop_debounce_n = Some(n.max(1));
        self
    }
    /// Configure the stop-early predictor.
    pub fn with_predictor(mut self, predictor: PredictorCfg) -> Self {
        self.predictor = Some(predictor);
        self
    }
    /// Provide a custom clock implementation; defaults to MonotonicClock when not provided.
    pub fn with_clock(mut self, clock: Box<dyn Clock + Send + Sync>) -> Self {
        self.clock = Some(clock);
        self
    }

    // Keep backward-compatible API used in tests; currently a no-op when None is passed.
    pub fn apply_calibration<C>(self, _src: Option<C>) -> Self {
        self
    }
}

// Setters that advance type-state when providing mandatory components
impl<M, T> DoserBuilder<Missing, M, T> {
    pub fn with_scale(self, scale: impl doser_traits::Scale + 'static) -> DoserBuilder<Set, M, T> {
        let DoserBuilder {
            scale: _,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            _calibration_loaded,
            estop_check,
            clock,
            estop_debounce_n,
            predictor,
            _s: _,
            _m: _,
            _t: _,
        } = self;
        DoserBuilder {
            scale: Some(Box::new(scale)),
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            _calibration_loaded,
            estop_check,
            clock,
            estop_debounce_n,
            predictor,
            _s: PhantomData,
            _m: PhantomData,
            _t: PhantomData,
        }
    }
}

impl<S, T> DoserBuilder<S, Missing, T> {
    pub fn with_motor(self, motor: impl doser_traits::Motor + 'static) -> DoserBuilder<S, Set, T> {
        let DoserBuilder {
            scale,
            motor: _,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            _calibration_loaded,
            estop_check,
            clock,
            estop_debounce_n,
            predictor,
            _s: _,
            _m: _,
            _t: _,
        } = self;
        DoserBuilder {
            scale,
            motor: Some(Box::new(motor)),
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            _calibration_loaded,
            estop_check,
            clock,
            estop_debounce_n,
            predictor,
            _s: PhantomData,
            _m: PhantomData,
            _t: PhantomData,
        }
    }
}

impl<S, M> DoserBuilder<S, M, Missing> {
    pub fn with_target_grams(self, grams: f32) -> DoserBuilder<S, M, Set> {
        let DoserBuilder {
            scale,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g: _,
            _calibration_loaded,
            estop_check,
            clock,
            estop_debounce_n,
            predictor,
            _s: _,
            _m: _,
            _t: _,
        } = self;
        DoserBuilder {
            scale,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g: Some(grams),
            _calibration_loaded,
            estop_check,
            clock,
            estop_debounce_n,
            predictor,
            _s: PhantomData,
            _m: PhantomData,
            _t: PhantomData,
        }
    }
}

impl DoserBuilder<Set, Set, Set> {
    /// Validate and build the Doser. Only available when Scale, Motor, and Target are set.
    pub fn build(self) -> Result<Doser> {
        self.try_build()
    }
}

/// Generic, statically-dispatched alias using the unified core.
pub type DoserG<S, M> = DoserCore<S, M>;

/// Build a generic, statically-dispatched DoserG from concrete scale and motor.
#[allow(clippy::too_many_arguments)]
pub fn build_doser<S, M>(
    scale: S,
    motor: M,
    filter: FilterCfg,
    control: ControlCfg,
    safety: SafetyCfg,
    timeouts: Timeouts,
    calibration: Option<Calibration>,
    target_g: f32,
    estop_check: Option<Box<dyn Fn() -> bool>>,
    predictor: Option<PredictorCfg>,
    clock: Option<Box<dyn Clock + Send + Sync>>,
    estop_debounce_n: Option<u8>,
) -> Result<DoserG<S, M>>
where
    S: doser_traits::Scale + 'static,
    M: doser_traits::Motor + 'static,
{
    if !(0.1..=5000.0).contains(&target_g) {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "target grams out of range",
        )));
    }
    if control.hysteresis_g.is_sign_negative() {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "hysteresis_g must be >= 0",
        )));
    }
    if control.slow_at_g.is_sign_negative() {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "slow_at_g must be >= 0",
        )));
    }
    if control.coarse_speed == 0 || control.fine_speed == 0 {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "motor speeds must be > 0",
        )));
    }
    if timeouts.sensor_ms == 0 {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "sensor_ms must be >= 1",
        )));
    }
    if safety.max_overshoot_g.is_sign_negative() {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "max_overshoot_g must be >= 0",
        )));
    }
    if safety.no_progress_epsilon_g.is_sign_negative() {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "no_progress_epsilon_g must be > 0",
        )));
    }
    if filter.sample_rate_hz == 0 {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "sample_rate_hz must be > 0",
        )));
    }
    let calibration = calibration.unwrap_or_default();
    let clock: Arc<dyn Clock + Send + Sync> = match clock {
        Some(b) => Arc::from(b),
        None => Arc::new(MonotonicClock::new()),
    };
    let estop_debounce_n = estop_debounce_n.unwrap_or(2);

    let ma_cap = filter.ma_window.max(1);
    let med_cap = filter.median_window.max(1);
    let epoch = clock.now();
    let now = clock.ms_since(epoch);
    let period_us = crate::util::period_us(filter.sample_rate_hz);
    let period_ms = period_us.div_ceil(1000);
    let to_cg = |g: f32| ((g * 100.0).round()) as i32;
    let target_cg = to_cg(target_g);
    let epsilon_cg = to_cg(control.epsilon_g);
    let max_overshoot_cg = to_cg(safety.max_overshoot_g);
    let no_progress_epsilon_cg = to_cg(safety.no_progress_epsilon_g);
    let slow_at_cg = to_cg(control.slow_at_g);
    // Sort bands descending and cache cg thresholds
    let mut control = control;
    if !control.speed_bands.is_empty() {
        control
            .speed_bands
            .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal));
    }
    let speed_bands_cg: Vec<(i32, u32)> = control
        .speed_bands
        .iter()
        .map(|(g, sps)| (to_cg(*g), *sps))
        .collect();

    // Precompute quantized calibration before moving `calibration` into the struct
    let cal_gain_cg_per_count = quantize_to_cg_i32(calibration.gain_g_per_count);
    let cal_offset_cg = quantize_to_cg_i32(calibration.offset_g);

    // Predictor: default disabled unless provided by caller.
    let predictor = predictor.unwrap_or_default();
    let pred_latency_ms = period_ms.saturating_add(predictor.extra_latency_ms);

    Ok(DoserG {
        scale,
        motor,
        filter,
        control,
        safety,
        timeouts,
        calibration,
        target_cg,
        clock,
        epoch,
        last_weight_cg: 0,
        settled_since_ms: None,
        start_ms: now,
        ma_buf: VecDeque::with_capacity(ma_cap),
        med_buf: VecDeque::with_capacity(med_cap),
        tmp_med_buf: Vec::with_capacity(med_cap),
        ema_prev_cg: None,
        period_us,
        cal_gain_cg_per_count,
        cal_offset_cg,
        slow_at_cg,
        epsilon_cg,
        max_overshoot_cg,
        no_progress_epsilon_cg,
        motor_started: false,
        estop_check,
        last_progress_cg: 0,
        last_progress_at_ms: now,
        estop_latched: false,
        estop_debounce_n,
        estop_count: 0,
        predictor,
        pred_hist: VecDeque::with_capacity(8),
        pred_latency_ms,
        speed_bands_cg,
        last_slope_ema_cg_per_ms: None,
        last_inflight_cg: None,
        early_stop_at_cg: None,
    })
}

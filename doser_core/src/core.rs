//! The unified dosing control loop (`DoserCore`).
//!
//! Contains the state machine that drives each iteration of the dosing process:
//! calibration caching, filtering (median + EMA/MA), speed selection, safety
//! watchdogs, predictive early stop, and settle detection.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use doser_traits::clock::Clock;
use eyre::WrapErr;

use crate::calibration::Calibration;
use crate::config::*;
use crate::error::{AbortReason, DoserError, Result};
use crate::fixed_point::{abs_diff_i32_u32, avg2_round_nearest_i32};
use crate::hw_error::map_hw_error;
use crate::status::DosingStatus;
use crate::util::div_round_nearest_i32;

/// Unified core for both dynamic (boxed) and generic (static dispatch) variants.
pub struct DoserCore<S: doser_traits::Scale, M: doser_traits::Motor> {
    pub(crate) scale: S,
    pub(crate) motor: M,
    pub(crate) filter: FilterCfg,
    pub(crate) control: ControlCfg,
    pub(crate) safety: SafetyCfg,
    pub(crate) timeouts: Timeouts,
    pub(crate) calibration: Calibration,
    pub(crate) target_cg: i32,
    pub(crate) clock: Arc<dyn Clock + Send + Sync>,
    pub(crate) epoch: Instant,

    pub(crate) last_weight_cg: i32,
    pub(crate) settled_since_ms: Option<u64>,
    pub(crate) start_ms: u64,
    pub(crate) ma_buf: VecDeque<i32>,
    pub(crate) med_buf: VecDeque<i32>,
    pub(crate) ema_prev_cg: Option<f32>,
    pub(crate) tmp_med_buf: Vec<i32>,
    pub(crate) period_us: u64,
    pub(crate) cal_gain_cg_per_count: i32,
    pub(crate) cal_offset_cg: i32,
    pub(crate) slow_at_cg: i32,
    pub(crate) epsilon_cg: i32,
    pub(crate) max_overshoot_cg: i32,
    pub(crate) no_progress_epsilon_cg: i32,
    pub(crate) motor_started: bool,
    pub(crate) estop_check: Option<Box<dyn Fn() -> bool>>,
    pub(crate) last_progress_cg: i32,
    pub(crate) last_progress_at_ms: u64,
    pub(crate) estop_latched: bool,
    pub(crate) estop_debounce_n: u8,
    pub(crate) estop_count: u8,
    pub(crate) predictor: PredictorCfg,
    pub(crate) pred_hist: VecDeque<(u64, i32)>,
    pub(crate) pred_latency_ms: u64,
    pub(crate) last_slope_ema_cg_per_ms: Option<f32>,
    pub(crate) last_inflight_cg: Option<i32>,
    pub(crate) early_stop_at_cg: Option<i32>,
    pub(crate) speed_bands_cg: Vec<(i32, u32)>,
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

    /// Return the configured filter parameters.
    pub fn filter_cfg(&self) -> &FilterCfg {
        &self.filter
    }

    /// Telemetry: last slope EMA in grams per second.
    pub fn last_slope_ema_gps(&self) -> Option<f32> {
        self.last_slope_ema_cg_per_ms.map(|v| v * 0.01 * 1000.0)
    }
    /// Telemetry: inflight mass estimate in grams.
    pub fn last_inflight_g(&self) -> Option<f32> {
        self.last_inflight_cg.map(|cg| (cg as f32) * 0.01)
    }
    /// Telemetry: weight at which predictor triggered early stop, in grams.
    pub fn early_stop_at_g(&self) -> Option<f32> {
        self.early_stop_at_cg.map(|cg| (cg as f32) * 0.01)
    }

    /// Process a pre-sampled raw reading (for sampler integration).
    pub fn step_from_raw(&mut self, raw: i32) -> Result<DosingStatus> {
        if self.estop_latched || self.poll_estop() {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on estop");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(AbortReason::Estop)));
        }
        let w_cg_raw = self.to_cg_cached(raw);
        let w_cg = self.apply_filter(w_cg_raw);
        self.process_weight(w_cg)
    }

    /// One iteration of the dosing loop (reads the scale internally).
    pub fn step(&mut self) -> Result<DosingStatus> {
        if self.estop_latched || self.poll_estop() {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on estop");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(AbortReason::Estop)));
        }

        let timeout = Duration::from_millis(self.timeouts.sensor_ms);
        let raw = self
            .scale
            .read(timeout)
            .map_err(|e| eyre::Report::new(map_hw_error(&*e)))
            .wrap_err("reading scale")?;

        let w_cg_raw = self.to_cg_cached(raw);
        let w_cg = self.apply_filter(w_cg_raw);
        self.process_weight(w_cg)
    }

    /// Reset per-run state. Call before a new dose.
    pub fn begin(&mut self) {
        self.epoch = self.clock.now();
        let now = self.clock.ms_since(self.epoch);
        self.start_ms = now;
        self.settled_since_ms = None;
        self.ma_buf.clear();
        self.med_buf.clear();
        self.ema_prev_cg = None;
        self.last_weight_cg = 0;
        self.motor_started = false;
        self.last_progress_cg = 0;
        self.last_progress_at_ms = now;
        self.estop_latched = false;
        self.estop_count = 0;
        self.pred_hist.clear();
        self.last_slope_ema_cg_per_ms = None;
        self.last_inflight_cg = None;
        self.early_stop_at_cg = None;
    }

    /// Stop the motor (best-effort).
    pub fn motor_stop(&mut self) -> Result<()> {
        self.motor
            .stop()
            .map_err(|e| eyre::Report::new(map_hw_error(&*e)))
            .wrap_err("motor_stop")
    }

    // ── Private: shared control loop logic ───────────────────────────────────

    /// Core weight-processing logic shared by `step()` and `step_from_raw()`.
    /// Handles safety checks, speed selection, settling, and motor commands.
    fn process_weight(&mut self, w_cg: i32) -> Result<DosingStatus> {
        self.last_weight_cg = w_cg;
        let err_cg = self.target_cg - w_cg;
        let abs_err_cg = err_cg.unsigned_abs();
        let now = self.clock.ms_since(self.epoch);

        // Safety: hard runtime cap
        if now.saturating_sub(self.start_ms) >= self.safety.max_run_ms {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on max-run cap");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::MaxRuntime,
            )));
        }

        // Safety: excessive overshoot guard
        if w_cg > self.target_cg + self.max_overshoot_cg {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed on overshoot");
            }
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::Overshoot,
            )));
        }

        // Predictive early stop to reduce overshoot under latency
        if self.maybe_early_stop(now, w_cg) {
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        }

        // Completion zone: target reached (within epsilon)
        if w_cg + self.epsilon_cg >= self.target_cg {
            if let Err(e) = self.motor_stop() {
                tracing::warn!(error = %e, "motor_stop failed entering settle zone");
            }
            if self.settled_since_ms.is_none() {
                self.settled_since_ms = Some(now);
            }
            if let Some(since) = self.settled_since_ms
                && now.saturating_sub(since) >= self.control.stable_ms
            {
                return Ok(DosingStatus::Complete);
            }
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        } else {
            self.settled_since_ms = None;
        }

        // Speed selection via bands or legacy fallback
        let target_speed = self.select_speed(err_cg, abs_err_cg);

        // No-progress watchdog
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

        // Motor commands
        if !self.motor_started {
            self.motor
                .start()
                .map_err(|e| eyre::Report::new(map_hw_error(&*e)))
                .wrap_err("motor start")?;
            self.motor_started = true;
        }
        self.motor
            .set_speed(target_speed)
            .map_err(|e| eyre::Report::new(map_hw_error(&*e)))
            .wrap_err("set_speed")?;

        self.clock.sleep(Duration::from_micros(self.period_us));
        Ok(DosingStatus::Running)
    }

    /// Select motor speed based on error magnitude.
    fn select_speed(&self, err_cg: i32, abs_err_cg: u32) -> u32 {
        if !self.speed_bands_cg.is_empty() {
            let mut selected_band: Option<(i32, u32)> = None;
            let mut target_speed = self.control.coarse_speed;
            let err_g = (err_cg.max(0) as f32) / 100.0;
            for (thr_cg, sps) in &self.speed_bands_cg {
                if err_cg >= *thr_cg {
                    selected_band = Some((*thr_cg, *sps));
                    target_speed = *sps;
                    break;
                }
            }
            if selected_band.is_none()
                && let Some((_thr_cg, sps)) = self.speed_bands_cg.last().copied()
            {
                selected_band = Some((_thr_cg, sps));
                target_speed = sps;
            }
            let thr_g = selected_band
                .map(|(cg, _)| (cg as f32) / 100.0)
                .unwrap_or(0.0);
            tracing::trace!(
                err_g,
                band_threshold_g = thr_g,
                band_sps = target_speed,
                "speed band select"
            );
            target_speed
        } else {
            // Legacy 2-speed proportional taper
            let target_speed = if self.slow_at_cg > 0 && abs_err_cg <= self.slow_at_cg as u32 {
                let ratio = (abs_err_cg as f32 / self.slow_at_cg as f32).clamp(0.0, 1.0);
                let min_frac = 0.2_f32;
                let frac = min_frac + (1.0 - min_frac) * ratio;
                ((self.control.fine_speed as f32 * frac).max(1.0)) as u32
            } else {
                self.control.coarse_speed
            };
            tracing::trace!(
                err_g = (err_cg.max(0) as f32) / 100.0,
                band_threshold_g = 0.0,
                band_sps = target_speed,
                "speed band select (legacy)"
            );
            target_speed
        }
    }

    #[inline]
    fn to_cg_cached(&self, raw: i32) -> i32 {
        let delta = raw.saturating_sub(self.calibration.zero_counts);
        self.cal_gain_cg_per_count
            .saturating_mul(delta)
            .saturating_add(self.cal_offset_cg)
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
        let med_win = self.filter.median_window.max(1);
        let ma_win = self.filter.ma_window.max(1);
        let ema_alpha = if self.filter.ema_alpha.is_finite() {
            self.filter.ema_alpha
        } else {
            0.0
        };

        // Median prefilter
        let after_median = if med_win > 1 {
            self.med_buf.push_back(w_cg);
            if self.med_buf.len() > med_win {
                self.med_buf.pop_front();
            }
            self.tmp_med_buf.clear();
            self.tmp_med_buf.extend(self.med_buf.iter().copied());
            self.tmp_med_buf.sort_unstable();
            let n = self.tmp_med_buf.len();
            #[cfg(debug_assertions)]
            {
                let med_len = self.med_buf.len();
                debug_assert!(n > 0, "median buffer unexpectedly empty");
                debug_assert_eq!(med_len, n);
                debug_assert!(n <= med_win, "median buffer exceeded window size");
            }
            let mid = n / 2;
            if n.is_multiple_of(2) {
                let a = self.tmp_med_buf[mid - 1];
                let b = self.tmp_med_buf[mid];
                avg2_round_nearest_i32(a, b)
            } else {
                self.tmp_med_buf[mid]
            }
        } else {
            w_cg
        };

        // Smoothing: EMA, Moving Average, or passthrough
        if ema_alpha > 0.0 {
            let x = after_median as f32;
            let alpha = ema_alpha.clamp(0.0, 1.0);
            let y = match self.ema_prev_cg {
                None => x,
                Some(prev) => alpha * x + (1.0 - alpha) * prev,
            };
            self.ema_prev_cg = Some(y);
            y.round() as i32
        } else if ma_win > 1 {
            self.ma_buf.push_back(after_median);
            if self.ma_buf.len() > ma_win {
                self.ma_buf.pop_front();
            }
            let sum_i128: i128 = self.ma_buf.iter().map(|&v| v as i128).sum();
            let len_i32 = self.ma_buf.len() as i32;
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

    /// Update predictor history and decide whether to stop early this iteration.
    #[inline]
    fn maybe_early_stop(&mut self, now_ms: u64, w_cg: i32) -> bool {
        if !self.predictor.enabled {
            return false;
        }
        // Gate on minimum progress
        if self.target_cg > 0 {
            let progress = (w_cg as f32) / (self.target_cg as f32);
            if progress < self.predictor.min_progress_ratio {
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

        let Some((t0, w0)) = self.pred_hist.front().copied() else {
            return false;
        };
        let dt_ms = now_ms.saturating_sub(t0);
        if dt_ms == 0 {
            return false;
        }
        let dw_cg = (w_cg as i64) - (w0 as i64);
        if dw_cg <= 0 {
            return false;
        }

        let num: i64 = dw_cg.saturating_mul(self.pred_latency_ms as i64);
        let den: i64 = (dt_ms as i64).max(1);
        let half = den >> 1;
        let inflight_i64 = if num >= 0 {
            (num + half) / den
        } else {
            (num - half) / den
        };
        let inflight_cg = inflight_i64.clamp(i32::MIN as i64, i32::MAX as i64) as i32;

        // Telemetry
        let slope_cg_per_ms = (dw_cg as f32) / (den as f32);
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

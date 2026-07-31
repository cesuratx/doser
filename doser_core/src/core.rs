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
use crate::config::{ControlCfg, FilterCfg, PredictorCfg, SafetyCfg, Timeouts};
use crate::error::{AbortReason, DoserError, Result};
use crate::fixed_point::{abs_diff_i32_u32, avg2_round_nearest_i32, cg_to_grams};
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
    pub(crate) cal_gain_scaled: i64,
    pub(crate) cal_offset_cg: i32,
    pub(crate) slow_at_cg: i32,
    pub(crate) epsilon_cg: i32,
    pub(crate) hysteresis_cg: i32,
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
        // Deliberately a summary: the full struct carries filter buffers and
        // predictor history that would swamp any log line.
        f.debug_struct("DoserCore")
            .field("target_g", &cg_to_grams(self.target_cg))
            .field("last_weight_g", &cg_to_grams(self.last_weight_cg))
            .field("motor_started", &self.motor_started)
            .finish_non_exhaustive()
    }
}

impl<S: doser_traits::Scale, M: doser_traits::Motor> DoserCore<S, M> {
    /// Return the last observed weight in grams.
    pub fn last_weight(&self) -> f32 {
        cg_to_grams(self.last_weight_cg)
    }

    /// Optionally set the tare baseline in raw counts.
    pub const fn set_tare_counts(&mut self, zero_counts: i32) {
        self.calibration.zero_counts = zero_counts;
    }

    /// Return the configured filter parameters.
    pub const fn filter_cfg(&self) -> &FilterCfg {
        &self.filter
    }

    /// Telemetry: last slope EMA in grams per second.
    pub fn last_slope_ema_gps(&self) -> Option<f32> {
        self.last_slope_ema_cg_per_ms.map(|v| v * 0.01 * 1000.0)
    }
    /// Telemetry: inflight mass estimate in grams.
    //
    // `i32 -> f32` has no infallible constructor; centigram weights stay far below 2^24
    // so the conversion is exact. The `* 0.01` form is preserved verbatim (rather than
    // routed through `cg_to_grams`) to keep the reported value bit-identical.
    #[allow(clippy::cast_precision_loss)]
    pub fn last_inflight_g(&self) -> Option<f32> {
        self.last_inflight_cg.map(|cg| (cg as f32) * 0.01)
    }
    /// Telemetry: weight at which predictor triggered early stop, in grams.
    #[allow(clippy::cast_precision_loss)] // see `last_inflight_g`
    pub fn early_stop_at_g(&self) -> Option<f32> {
        self.early_stop_at_cg.map(|cg| (cg as f32) * 0.01)
    }

    /// Process a pre-sampled raw reading (for sampler integration).
    pub fn step_from_raw(&mut self, raw: i32) -> Result<DosingStatus> {
        if self.estop_latched || self.poll_estop() {
            self.motor_stop_best_effort("estop");
            return Ok(DosingStatus::Aborted(DoserError::Abort(AbortReason::Estop)));
        }
        let w_cg_raw = self.to_cg_cached(raw);
        let w_cg = self.apply_filter(w_cg_raw);
        self.process_weight(w_cg)
    }

    /// One iteration of the dosing loop (reads the scale internally).
    pub fn step(&mut self) -> Result<DosingStatus> {
        if self.estop_latched || self.poll_estop() {
            self.motor_stop_best_effort("estop");
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

    /// Stop the motor, returning any hardware error (used on the success path).
    ///
    /// Clears `motor_started` for the same reason as
    /// [`Self::motor_stop_best_effort`] — see the restartability note there.
    pub fn motor_stop(&mut self) -> Result<()> {
        self.motor_started = false;
        self.motor
            .stop()
            .map_err(|e| eyre::Report::new(map_hw_error(&*e)))
            .wrap_err("motor_stop")
    }

    /// Best-effort motor stop for safety/abort paths.
    ///
    /// Unlike [`Self::motor_stop`], this never returns an error: the caller is
    /// already aborting, so the goal is maximum effort to de-energize. It retries
    /// a bounded number of times and escalates to an error-level log if every
    /// attempt fails, so a stuck motor is loud rather than silently ignored.
    ///
    /// Clearing `motor_started` is what makes the motor restartable *within* a
    /// run. Backends resume stepping only on `start()` (the hardware motor's
    /// stepping thread gates on a `running` flag set there; the sim scale only
    /// accumulates while running), so `set_speed()` alone never restores flow.
    /// With the flag left set, a later reading that falls back below
    /// `target - epsilon` — a sensor-noise dip out of the settle band, or a
    /// predictor early stop whose in-flight estimate was too generous so the
    /// weight plateaus short of target — would fall through to speed selection
    /// and command speeds at a stopped motor until the no-progress watchdog
    /// aborted a nearly-complete dose.
    fn motor_stop_best_effort(&mut self, ctx: &'static str) {
        const MAX_ATTEMPTS: u32 = 3;
        self.motor_started = false;
        for attempt in 1..=MAX_ATTEMPTS {
            match self.motor.stop() {
                Ok(()) => return,
                Err(e) => {
                    if attempt == MAX_ATTEMPTS {
                        tracing::error!(
                            error = %e,
                            ctx,
                            attempts = attempt,
                            "motor stop FAILED on abort path; motor may still be energized"
                        );
                    } else {
                        tracing::warn!(error = %e, ctx, attempt, "motor stop failed; retrying");
                    }
                }
            }
        }
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
            self.motor_stop_best_effort("max-run cap");
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::MaxRuntime,
            )));
        }

        // Safety: excessive overshoot guard
        if w_cg > self.target_cg + self.max_overshoot_cg {
            self.motor_stop_best_effort("overshoot");
            return Ok(DosingStatus::Aborted(DoserError::Abort(
                AbortReason::Overshoot,
            )));
        }

        // Predictive early stop to reduce overshoot under latency
        if self.maybe_early_stop(now, w_cg) {
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        }

        // Completion zone: target reached (within epsilon). The motor is stopped and
        // the weight must hold the zone for `stable_ms` before completion is declared;
        // a reading that leaves the zone downward clears the timer in the `else` arm
        // below and restarts the motor to top the dose up.
        if w_cg + self.epsilon_cg >= self.target_cg {
            self.motor_stop_best_effort("entering settle zone");
            // The settle timer starts on zone entry and is only ever *restarted* by a
            // reading that leaves the acceptance band on the LOW side. Restarting (rather
            // than clearing) preserves the invariant that `stable_ms == 0` completes as
            // soon as the completion zone is entered.
            //
            // A reading *above* the band must NOT restart the timer. Over-delivery is
            // irreversible — an auger cannot remove mass, the motor is already stopped
            // here, and this branch returns before the motor-command section — so
            // restarting on a high reading livelocks: the timer resets on every
            // subsequent sample, the run burns the whole `max_run_ms`, and a finished,
            // slightly over-delivered dose is misreported as `MaxRuntime`. Beans still in
            // flight when the motor stops make that the ordinary case, not a corner case.
            // Anything above the band but within `target + max_overshoot_g` is inside the
            // tolerance the operator declared, and the overshoot guard above has already
            // aborted anything beyond it, so settling and completing is correct.
            //
            // NOTE (design, not a bug): `below_band` is currently unreachable. The zone
            // opens at `target - epsilon` while the band opens at
            // `target - max(hysteresis, epsilon)`, which is at or below it, so every
            // in-zone reading is at or above the band's lower edge. A dip deep enough to
            // leave the band has already left the zone and taken the `else` arm. The
            // guard is kept because it is the correct condition if either edge is ever
            // re-parameterised — but it does mean `control.hysteresis_g` has no effect on
            // completion today. Making it matter needs a real settle test (weight stopped
            // *changing* by more than the band, rather than sitting within the band of the
            // target), which is a control-law change for the owner to make deliberately.
            let band_cg = self.hysteresis_cg.max(self.epsilon_cg).unsigned_abs();
            let below_band = abs_err_cg > band_cg && w_cg < self.target_cg;
            match self.settled_since_ms {
                None => self.settled_since_ms = Some(now),
                Some(_) if below_band => self.settled_since_ms = Some(now),
                Some(_) => {}
            }
            if let Some(since) = self.settled_since_ms
                && now.saturating_sub(since) >= self.control.stable_ms
            {
                return Ok(DosingStatus::Complete);
            }
            self.clock.sleep(Duration::from_micros(self.period_us));
            return Ok(DosingStatus::Running);
        }
        self.settled_since_ms = None;

        // Speed selection via bands or legacy fallback
        let target_speed = self.select_speed(err_cg, abs_err_cg);

        // No-progress watchdog
        if self.safety.no_progress_ms > 0 && self.no_progress_epsilon_cg > 0 && target_speed > 0 {
            let progress_delta_cg = abs_diff_i32_u32(w_cg, self.last_progress_cg);
            if progress_delta_cg >= self.no_progress_epsilon_cg.cast_unsigned() {
                self.last_progress_cg = w_cg;
                self.last_progress_at_ms = now;
            } else if now.saturating_sub(self.last_progress_at_ms) >= self.safety.no_progress_ms {
                self.motor_stop_best_effort("no-progress watchdog");
                return Ok(DosingStatus::Aborted(DoserError::Abort(
                    AbortReason::NoProgress,
                )));
            }
        }

        // Motor commands. Reaching here after a deliberate stop (settle zone or
        // predictor early stop) restarts the motor to top the dose up.
        //
        // Why this converges instead of flapping: a stop only happens after the
        // weight rose — into the completion zone, or by `dw > 0` over the whole
        // predictor window — and a restart only happens after it left again, so
        // each cycle is separated by real, observed mass movement rather than by a
        // single noisy sample. Successive stop points therefore climb monotonically
        // toward the target until the weight stays inside the acceptance band.
        // If a restart yields no mass, nothing can stop the motor again (the
        // predictor needs `dw > 0`, the completion zone needs the weight to rise),
        // so the no-progress watchdog above runs uninterrupted from the restart and
        // fires; the max-run cap and overshoot guard bound the run either way.
        if !self.motor_started {
            self.motor
                .start()
                .map_err(|e| eyre::Report::new(map_hw_error(&*e)))
                .wrap_err("motor start")?;
            self.motor_started = true;
            // The watchdog measures "no mass while the motor is commanded to run"
            // (see `SafetyCfg::no_progress_ms`), so time spent deliberately stopped
            // must not count against the restarted motor. Resets here are always
            // backed by observed movement, per the argument above.
            self.last_progress_cg = w_cg;
            self.last_progress_at_ms = now;
        }
        self.motor
            .set_speed(target_speed)
            .map_err(|e| eyre::Report::new(map_hw_error(&*e)))
            .wrap_err("set_speed")?;

        self.clock.sleep(Duration::from_micros(self.period_us));
        Ok(DosingStatus::Running)
    }

    /// Select motor speed based on error magnitude.
    //
    // Lint rationale, all confined to the speed taper:
    // * `similar_names`: `err_cg`/`err_g` and `thr_cg`/`thr_g` are this crate's unit
    //   suffixes; dropping them would lose the unit discipline the loop depends on.
    // * `cast_*`: the taper is the one place the integer control law drops into f32 to
    //   interpolate. `i32/u32 -> f32` have no infallible constructors, and the final
    //   `f32 -> u32` step rate is already floored at 1.0 and clamped by the backends.
    // * `suboptimal_flops`: `mul_add` is a fused op and would change the taper's
    //   rounding. This picks the motor's step rate — leave the arithmetic alone.
    #[allow(
        clippy::similar_names,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::suboptimal_flops
    )]
    fn select_speed(&self, err_cg: i32, abs_err_cg: u32) -> u32 {
        if self.speed_bands_cg.is_empty() {
            // Legacy 2-speed proportional taper
            let target_speed =
                if self.slow_at_cg > 0 && abs_err_cg <= self.slow_at_cg.cast_unsigned() {
                    let ratio = (abs_err_cg as f32 / self.slow_at_cg as f32).clamp(0.0, 1.0);
                    let min_frac = 0.2_f32;
                    let frac = min_frac + (1.0 - min_frac) * ratio;
                    ((self.control.fine_speed as f32 * frac).max(1.0)) as u32
                } else {
                    self.control.coarse_speed
                };
            tracing::trace!(
                err_g = cg_to_grams(err_cg.max(0)),
                band_threshold_g = 0.0,
                band_sps = target_speed,
                "speed band select (legacy)"
            );
            target_speed
        } else {
            let mut selected_band: Option<(i32, u32)> = None;
            let mut target_speed = self.control.coarse_speed;
            let err_g = cg_to_grams(err_cg.max(0));
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
            let thr_g = selected_band.map_or(0.0, |(cg, _)| cg_to_grams(cg));
            tracing::trace!(
                err_g,
                band_threshold_g = thr_g,
                band_sps = target_speed,
                "speed band select"
            );
            target_speed
        }
    }

    #[inline]
    fn to_cg_cached(&self, raw: i32) -> i32 {
        let delta = i64::from(raw) - i64::from(self.calibration.zero_counts);
        crate::fixed_point::cg_from_delta_scaled(delta, self.cal_gain_scaled, self.cal_offset_cg)
    }

    /// Out-of-band E-stop poll for orchestrators (e.g. the sampler runner).
    ///
    /// In sampler mode the control loop only runs `step_from_raw` when a sample
    /// arrives, so E-stop response would otherwise be coupled to sensor latency
    /// (up to the read timeout). Calling this each orchestration iteration keeps
    /// the response time bounded by the loop period instead. Returns true if
    /// E-stop is (or becomes) latched, stopping the motor best-effort.
    pub fn poll_estop_stop(&mut self) -> bool {
        if self.estop_latched || self.poll_estop() {
            self.motor_stop_best_effort("estop (out-of-band)");
            true
        } else {
            false
        }
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

    // Lint rationale: the EMA is the one filter stage that works in f32.
    // `i32 -> f32` has no infallible constructor and the round-trip back to
    // centigrams is an explicit `round()`; `mul_add` is fused and would change the
    // filter's rounding, so the EMA recurrence stays written out.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::suboptimal_flops
    )]
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
            let n = self.tmp_med_buf.len();
            #[cfg(debug_assertions)]
            {
                let med_len = self.med_buf.len();
                debug_assert!(n > 0, "median buffer unexpectedly empty");
                debug_assert_eq!(med_len, n);
                debug_assert!(n <= med_win, "median buffer exceeded window size");
            }
            let mid = n / 2;
            // O(n) selection rather than a full O(n log n) sort: same result, less
            // per-sample work and jitter on the control loop.
            if n.is_multiple_of(2) {
                let (lo, mid_val, _) = self.tmp_med_buf.select_nth_unstable(mid);
                let mid_val = *mid_val;
                // Even window: the lower-middle order statistic is the max of the
                // lower partition (non-empty since mid >= 1 when n is even and > 0).
                let lower = lo.iter().copied().max().unwrap_or(mid_val);
                avg2_round_nearest_i32(lower, mid_val)
            } else {
                *self.tmp_med_buf.select_nth_unstable(mid).1
            }
        } else {
            w_cg
        };

        // Smoothing: EMA, Moving Average, or passthrough
        if ema_alpha > 0.0 {
            let x = after_median as f32;
            let alpha = ema_alpha.clamp(0.0, 1.0);
            let y = self
                .ema_prev_cg
                .map_or(x, |prev| alpha * x + (1.0 - alpha) * prev);
            self.ema_prev_cg = Some(y);
            y.round() as i32
        } else if ma_win > 1 {
            self.ma_buf.push_back(after_median);
            if self.ma_buf.len() > ma_win {
                self.ma_buf.pop_front();
            }
            let sum_i128: i128 = self.ma_buf.iter().map(|&v| i128::from(v)).sum();
            // The builder caps window sizes at MAX_WINDOW, so the length always fits.
            let len_i32 = i32::try_from(self.ma_buf.len()).unwrap_or(i32::MAX);
            if len_i32 > 0 {
                i32::try_from(sum_i128).map_or_else(
                    // The running sum left i32 range: divide in i128 instead.
                    |_| {
                        let n = i128::from(len_i32);
                        let q = if sum_i128 >= 0 {
                            (sum_i128 + n / 2) / n
                        } else {
                            (sum_i128 - n / 2) / n
                        };
                        #[cfg(debug_assertions)]
                        debug_assert!(
                            (i128::from(i32::MIN)..=i128::from(i32::MAX)).contains(&q),
                            "moving-average result out of i32 range"
                        );
                        // A mean of i32 samples is always back in i32 range.
                        i32::try_from(q)
                            .unwrap_or_else(|_| if q.is_negative() { i32::MIN } else { i32::MAX })
                    },
                    |sum_i32| div_round_nearest_i32(sum_i32, len_i32),
                )
            } else {
                0
            }
        } else {
            after_median
        }
    }

    /// Update predictor history and decide whether to stop early this iteration.
    //
    // Lint rationale: the inflight estimate is exact integer math (`i64`), and only the
    // progress ratio and the slope telemetry drop into f32 — `i32`/`i64 -> f32` have no
    // infallible constructor. `mul_add` is fused and would change the slope EMA's
    // rounding, so that recurrence is left as written.
    #[inline]
    #[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
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
        let dw_cg = i64::from(w_cg) - i64::from(w0);
        if dw_cg <= 0 {
            return false;
        }

        let num: i64 = dw_cg.saturating_mul(self.pred_latency_ms.cast_signed());
        let den: i64 = dt_ms.cast_signed().max(1);
        let half = den >> 1;
        let inflight_i64 = if num >= 0 {
            (num + half) / den
        } else {
            (num - half) / den
        };
        let inflight_cg = i32::try_from(inflight_i64).unwrap_or_else(|_| {
            if inflight_i64.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }
        });

        // Telemetry
        let slope_cg_per_ms = (dw_cg as f32) / (den as f32);
        let alpha = if self.filter.ema_alpha.is_finite() && self.filter.ema_alpha > 0.0 {
            self.filter.ema_alpha
        } else {
            0.3
        };
        self.last_slope_ema_cg_per_ms = Some(
            self.last_slope_ema_cg_per_ms
                .map_or(slope_cg_per_ms, |prev| {
                    alpha * slope_cg_per_ms + (1.0 - alpha) * prev
                }),
        );
        self.last_inflight_cg = Some(inflight_cg);

        let predicted = w_cg
            .saturating_add(inflight_cg)
            .saturating_add(self.epsilon_cg);
        if predicted >= self.target_cg {
            self.motor_stop_best_effort("predictor early-stop");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ControlCfg, FilterCfg, PredictorCfg, SafetyCfg, Timeouts};
    use std::error::Error;
    use std::sync::Mutex;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        Start,
        SetSpeed(u32),
        Stop,
    }

    /// Motor that records the exact command sequence it received.
    #[derive(Clone, Default)]
    struct RecordingMotor {
        ops: Arc<Mutex<Vec<Op>>>,
    }
    impl RecordingMotor {
        fn ops(&self) -> Vec<Op> {
            self.ops.lock().unwrap().clone()
        }
    }
    impl doser_traits::Motor for RecordingMotor {
        fn start(&mut self) -> core::result::Result<(), Box<dyn Error + Send + Sync>> {
            self.ops.lock().unwrap().push(Op::Start);
            Ok(())
        }
        fn set_speed(
            &mut self,
            sps: u32,
        ) -> core::result::Result<(), Box<dyn Error + Send + Sync>> {
            self.ops.lock().unwrap().push(Op::SetSpeed(sps));
            Ok(())
        }
        fn stop(&mut self) -> core::result::Result<(), Box<dyn Error + Send + Sync>> {
            self.ops.lock().unwrap().push(Op::Stop);
            Ok(())
        }
    }

    /// Scale returning a fixed sequence, then repeating the last value.
    struct SeqScale {
        seq: Vec<i32>,
        idx: usize,
    }
    impl doser_traits::Scale for SeqScale {
        fn read(
            &mut self,
            _t: Duration,
        ) -> core::result::Result<i32, Box<dyn Error + Send + Sync>> {
            let v = self
                .seq
                .get(self.idx)
                .copied()
                .unwrap_or_else(|| self.seq.last().copied().unwrap_or(0));
            self.idx += 1;
            Ok(v)
        }
    }

    /// Deterministic clock: `sleep` advances a virtual offset instead of blocking.
    #[derive(Clone)]
    struct ManualClock {
        origin: Instant,
        offset: Arc<Mutex<Duration>>,
    }
    impl ManualClock {
        fn new() -> Self {
            Self {
                origin: Instant::now(),
                offset: Arc::new(Mutex::new(Duration::ZERO)),
            }
        }
    }
    impl Clock for ManualClock {
        fn now(&self) -> Instant {
            self.origin + *self.offset.lock().unwrap()
        }
        fn sleep(&self, d: Duration) {
            *self.offset.lock().unwrap() += d;
        }
    }

    /// `gain = 1.0 g/count` makes raw counts equal grams, for readable values.
    fn unit_cal() -> Calibration {
        Calibration {
            gain_g_per_count: 1.0,
            zero_counts: 0,
            offset_g: 0.0,
        }
    }

    fn build(
        seq: Vec<i32>,
        motor: RecordingMotor,
        predictor: PredictorCfg,
    ) -> crate::DoserG<SeqScale, RecordingMotor> {
        crate::build_doser(
            SeqScale { seq, idx: 0 },
            motor,
            FilterCfg {
                ma_window: 1,
                median_window: 1,
                sample_rate_hz: 100, // 10 ms period
                ema_alpha: 0.0,
            },
            ControlCfg {
                speed_bands: vec![],
                ..ControlCfg::default()
            },
            SafetyCfg::default(), // no-progress watchdog disabled
            Timeouts { sensor_ms: 5 },
            Some(unit_cal()),
            10.0,
            None,
            Some(predictor),
            Some(Box::new(ManualClock::new())),
            None,
        )
        .expect("build doser")
    }

    /// Every `set_speed` must reach a *running* motor: backends only resume
    /// stepping on `start()`, so a speed command issued after a stop is lost.
    fn assert_no_speed_while_stopped(ops: &[Op]) {
        let mut running = false;
        for op in ops {
            match op {
                Op::Start => running = true,
                Op::Stop => running = false,
                Op::SetSpeed(sps) => {
                    assert!(
                        running || *sps == 0,
                        "set_speed({sps}) issued to a stopped motor: {ops:?}"
                    );
                }
            }
        }
    }

    fn count_starts(ops: &[Op]) -> usize {
        ops.iter().filter(|o| matches!(o, Op::Start)).count()
    }

    #[test]
    fn motor_restarts_after_dip_out_of_the_settle_band() {
        // 5 -> 8 -> 10 g (enters the completion zone, motor stops), then a dip to
        // 9 g. The dose must be topped up rather than commanding speeds at a
        // stopped motor until the run fails.
        let motor = RecordingMotor::default();
        let mut doser = build(vec![5, 8, 10, 9], motor.clone(), PredictorCfg::default());
        doser.begin();
        for _ in 0..4 {
            assert!(matches!(
                doser.step().expect("step ok"),
                DosingStatus::Running
            ));
        }
        let ops = motor.ops();
        assert_eq!(
            count_starts(&ops),
            2,
            "motor must restart after the dip: {ops:?}"
        );
        assert_no_speed_while_stopped(&ops);
    }

    #[test]
    fn dose_completes_after_topping_up_from_a_dip() {
        // Same dip, but the top-up brings the weight back to target and holds it.
        let motor = RecordingMotor::default();
        let mut doser = build(
            vec![5, 8, 10, 9, 10],
            motor.clone(),
            PredictorCfg::default(),
        );
        doser.begin();
        let mut status = DosingStatus::Running;
        for _ in 0..64 {
            status = doser.step().expect("step ok");
            if !matches!(status, DosingStatus::Running) {
                break;
            }
        }
        assert!(matches!(status, DosingStatus::Complete), "got {status:?}");
        let ops = motor.ops();
        assert_eq!(
            count_starts(&ops),
            2,
            "motor must restart after the dip: {ops:?}"
        );
        assert_no_speed_while_stopped(&ops);
    }

    #[test]
    fn motor_restarts_after_a_predictor_early_stop_that_plateaus_short() {
        // A rising ramp makes the predictor early-stop at 6 g (its in-flight
        // estimate over-predicts), then the weight plateaus short of the 10 g
        // target. Once the slope decays to zero the predictor stops holding the
        // motor off, and the loop must restart it to finish the dose.
        let motor = RecordingMotor::default();
        let predictor = PredictorCfg {
            enabled: true,
            window: 6,
            extra_latency_ms: 1_000, // grossly over-estimates in-flight mass
            min_progress_ratio: 0.10,
        };
        let mut doser = build(vec![1, 2, 3, 4, 5, 6], motor.clone(), predictor);
        doser.begin();
        for _ in 0..24 {
            assert!(matches!(
                doser.step().expect("step ok"),
                DosingStatus::Running
            ));
        }
        let ops = motor.ops();
        assert!(
            ops.contains(&Op::Stop),
            "predictor should have early-stopped: {ops:?}"
        );
        assert_eq!(
            count_starts(&ops),
            2,
            "motor must restart once the plateau clears the predictor: {ops:?}"
        );
        assert_no_speed_while_stopped(&ops);
    }
}

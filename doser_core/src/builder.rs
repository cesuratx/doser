//! Type-state builder for `Doser` and generic `build_doser` constructor.
//!
//! The builder enforces at compile time that Scale, Motor, and Target are provided
//! before `build()` is available. `try_build()` is always available for dynamic checks.

use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::Arc;

use doser_traits::clock::{Clock, MonotonicClock};

use crate::calibration::Calibration;
use crate::config::*;
use crate::core::DoserCore;
use crate::error::{BuildError, Result};
use crate::fixed_point::{grams_to_cg, quantize_to_cg_i32};
use crate::status::DosingStatus;

// ── Public dynamic-dispatch wrapper ──────────────────────────────────────────

/// Public dynamic (boxed) doser that preserves the existing API via composition.
pub struct Doser {
    pub(crate) inner: DoserCore<Box<dyn doser_traits::Scale>, Box<dyn doser_traits::Motor>>,
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

    /// Return the configured filter parameters.
    pub fn filter_cfg(&self) -> &FilterCfg {
        self.inner.filter_cfg()
    }

    /// Process a pre-sampled raw reading (for sampler integration).
    pub fn step_from_raw(&mut self, raw: i32) -> Result<DosingStatus> {
        self.inner.step_from_raw(raw)
    }

    /// Reset per-run state. Call before a new dose.
    pub fn begin(&mut self) {
        self.inner.begin();
    }

    /// One iteration of the dosing loop.
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

// ── Type-state markers ───────────────────────────────────────────────────────

pub struct Missing;
pub struct Set;

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
    estop_check: Option<Box<dyn Fn() -> bool>>,
    clock: Option<Box<dyn Clock + Send + Sync>>,
    estop_debounce_n: Option<u8>,
    predictor: Option<PredictorCfg>,
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

/// Validate configuration and construct a `DoserCore` with precomputed caches.
///
/// This is the single source of truth for validation and construction,
/// used by both `DoserBuilder::try_build()` and `build_doser()`.
#[allow(clippy::too_many_arguments)]
fn validate_and_build<S: doser_traits::Scale, M: doser_traits::Motor>(
    scale: S,
    motor: M,
    filter: FilterCfg,
    mut control: ControlCfg,
    safety: SafetyCfg,
    timeouts: Timeouts,
    calibration: Calibration,
    target_g: f32,
    estop_check: Option<Box<dyn Fn() -> bool>>,
    predictor: PredictorCfg,
    clock: Option<Box<dyn Clock + Send + Sync>>,
    estop_debounce_n: u8,
) -> Result<DoserCore<S, M>> {
    // ── Validation ───────────────────────────────────────────────────────────
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
            "no_progress_epsilon_g must be >= 0",
        )));
    }
    if filter.sample_rate_hz == 0 {
        return Err(eyre::Report::new(BuildError::InvalidConfig(
            "sample_rate_hz must be > 0",
        )));
    }
    // Validate speed band entries
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

    // ── Precompute ───────────────────────────────────────────────────────────
    let ma_cap = filter.ma_window.max(1);
    let med_cap = filter.median_window.max(1);

    let clock: Arc<dyn Clock + Send + Sync> = match clock {
        Some(b) => Arc::from(b),
        None => Arc::new(MonotonicClock::new()),
    };

    let epoch = clock.now();
    let now = clock.ms_since(epoch);

    let period_us = crate::util::period_us(filter.sample_rate_hz);
    let period_ms = period_us.div_ceil(1000);
    let pred_latency_ms = period_ms.saturating_add(predictor.extra_latency_ms);

    // Sort speed bands descending by threshold
    if !control.speed_bands.is_empty() {
        control.speed_bands.sort_by(|a, b| b.0.total_cmp(&a.0));
    }

    let target_cg = grams_to_cg(target_g);
    let epsilon_cg = grams_to_cg(control.epsilon_g);
    let max_overshoot_cg = grams_to_cg(safety.max_overshoot_g);
    let no_progress_epsilon_cg = grams_to_cg(safety.no_progress_epsilon_g);
    let slow_at_cg = grams_to_cg(control.slow_at_g);
    let speed_bands_cg: Vec<(i32, u32)> = control
        .speed_bands
        .iter()
        .map(|(g, sps)| (grams_to_cg(*g), *sps))
        .collect();

    let cal_gain_cg_per_count = quantize_to_cg_i32(calibration.gain_g_per_count);
    let cal_offset_cg = quantize_to_cg_i32(calibration.offset_g);

    Ok(DoserCore {
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

impl<S, M, T> DoserBuilder<S, M, T> {
    /// Fallible build available in any type-state; returns detailed error for missing pieces.
    pub fn try_build(self) -> Result<Doser> {
        let scale = self
            .scale
            .ok_or_else(|| eyre::Report::new(BuildError::MissingScale))?;
        let motor = self
            .motor
            .ok_or_else(|| eyre::Report::new(BuildError::MissingMotor))?;
        let target_g = self
            .target_g
            .ok_or_else(|| eyre::Report::new(BuildError::MissingTarget))?;

        let inner = validate_and_build(
            scale,
            motor,
            self.filter.unwrap_or_default(),
            self.control.unwrap_or_default(),
            self.safety.unwrap_or_default(),
            self.timeouts.unwrap_or_default(),
            self.calibration.unwrap_or_default(),
            target_g,
            self.estop_check,
            self.predictor.unwrap_or_default(),
            self.clock,
            self.estop_debounce_n.unwrap_or(2),
        )?;

        Ok(Doser { inner })
    }
}

/// Chainable setters that do not affect type-state.
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
    /// Provide a custom clock implementation; defaults to `MonotonicClock` when not provided.
    pub fn with_clock(mut self, clock: Box<dyn Clock + Send + Sync>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Backward-compatible no-op when `None` is passed.
    pub fn apply_calibration<C>(self, _src: Option<C>) -> Self {
        self
    }
}

// Setters that advance type-state
impl<M, T> DoserBuilder<Missing, M, T> {
    pub fn with_scale(self, scale: impl doser_traits::Scale + 'static) -> DoserBuilder<Set, M, T> {
        DoserBuilder {
            scale: Some(Box::new(scale)),
            motor: self.motor,
            filter: self.filter,
            control: self.control,
            safety: self.safety,
            timeouts: self.timeouts,
            calibration: self.calibration,
            target_g: self.target_g,
            _calibration_loaded: self._calibration_loaded,
            estop_check: self.estop_check,
            clock: self.clock,
            estop_debounce_n: self.estop_debounce_n,
            predictor: self.predictor,
            _s: PhantomData,
            _m: PhantomData,
            _t: PhantomData,
        }
    }
}

impl<S, T> DoserBuilder<S, Missing, T> {
    pub fn with_motor(self, motor: impl doser_traits::Motor + 'static) -> DoserBuilder<S, Set, T> {
        DoserBuilder {
            scale: self.scale,
            motor: Some(Box::new(motor)),
            filter: self.filter,
            control: self.control,
            safety: self.safety,
            timeouts: self.timeouts,
            calibration: self.calibration,
            target_g: self.target_g,
            _calibration_loaded: self._calibration_loaded,
            estop_check: self.estop_check,
            clock: self.clock,
            estop_debounce_n: self.estop_debounce_n,
            predictor: self.predictor,
            _s: PhantomData,
            _m: PhantomData,
            _t: PhantomData,
        }
    }
}

impl<S, M> DoserBuilder<S, M, Missing> {
    pub fn with_target_grams(self, grams: f32) -> DoserBuilder<S, M, Set> {
        DoserBuilder {
            scale: self.scale,
            motor: self.motor,
            filter: self.filter,
            control: self.control,
            safety: self.safety,
            timeouts: self.timeouts,
            calibration: self.calibration,
            target_g: Some(grams),
            _calibration_loaded: self._calibration_loaded,
            estop_check: self.estop_check,
            clock: self.clock,
            estop_debounce_n: self.estop_debounce_n,
            predictor: self.predictor,
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

/// Build a generic, statically-dispatched `DoserG` from concrete scale and motor.
///
/// Delegates to the shared `validate_and_build` — no duplicated validation logic.
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
    validate_and_build(
        scale,
        motor,
        filter,
        control,
        safety,
        timeouts,
        calibration.unwrap_or_default(),
        target_g,
        estop_check,
        predictor.unwrap_or_default(),
        clock,
        estop_debounce_n.unwrap_or(2),
    )
}

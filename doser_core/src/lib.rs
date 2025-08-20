//! Core dosing logic (hardware-agnostic).
//! - Keeps all hardware behind doser_traits::Scale/Motor
//! - Exposes a small builder to wire config + traits
//! - One-step control loop (call step() from the CLI or a strategy)

pub mod error;
pub mod runner;
pub mod sampler;

use crate::error::BuildError;
use crate::error::{DoserError, Result};
use doser_traits::clock::{Clock, MonotonicClock};
use eyre::WrapErr;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

// For typed hardware error mapping
use doser_hardware::error::HwError;

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
}

impl Default for FilterCfg {
    fn default() -> Self {
        Self {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
        }
    }
}

/// Control configuration.
#[derive(Debug, Clone)]
pub struct ControlCfg {
    /// Switch to fine speed once err <= slow_at_g
    pub slow_at_g: f32,
    /// Consider “in band” if |err| <= hysteresis_g
    pub hysteresis_g: f32,
    /// Reported stable if weight stays within hysteresis for this many ms
    pub stable_ms: u64,
    /// Coarse motor speed (steps per second or implementation-defined)
    pub coarse_speed: u32,
    /// Fine motor speed for the final approach
    pub fine_speed: u32,
    /// Additional tolerance below target (grams) to consider completion zone
    pub epsilon_g: f32,
}

impl Default for ControlCfg {
    fn default() -> Self {
        Self {
            slow_at_g: 1.0,
            hysteresis_g: 0.05,
            stable_ms: 250,
            coarse_speed: 1200,
            fine_speed: 250,
            epsilon_g: 0.0,
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
    // Temporary preallocated buffer to compute medians without per-step allocation
    tmp_med_buf: Vec<i32>,
    // Cached control-loop sleep period in microseconds to avoid repeated division
    period_us: u64,
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
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State("estop".into())));
        }
        let w_cg_raw = ((self.calibration.to_grams(raw) * 100.0).round()) as i32;
        let w_cg = self.apply_filter(w_cg_raw);
        self.last_weight_cg = w_cg;
        let err_cg = self.target_cg - w_cg;
        let abs_err_cg = err_cg.unsigned_abs() as i32;
        let now = self.clock.ms_since(self.epoch);
        if now.saturating_sub(self.start_ms) >= self.safety.max_run_ms {
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State(
                "max run time exceeded".into(),
            )));
        }
        if w_cg > self.target_cg + self.max_overshoot_cg {
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State(
                "max overshoot exceeded".into(),
            )));
        }
        if w_cg + self.epsilon_cg >= self.target_cg {
            let _ = self.motor_stop();
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
        let target_speed = if abs_err_cg <= self.slow_at_cg && self.slow_at_cg > 0 {
            let ratio = (abs_err_cg as f32 / self.slow_at_cg as f32).clamp(0.0, 1.0);
            let min_frac = 0.2_f32;
            let frac = min_frac + (1.0 - min_frac) * ratio;
            ((self.control.fine_speed as f32 * frac).max(1.0)) as u32
        } else {
            self.control.coarse_speed
        };
        if self.safety.no_progress_ms > 0 && self.no_progress_epsilon_cg > 0 && target_speed > 0 {
            if (w_cg - self.last_progress_cg).unsigned_abs() as i32 >= self.no_progress_epsilon_cg {
                self.last_progress_cg = w_cg;
                self.last_progress_at_ms = now;
            } else if now.saturating_sub(self.last_progress_at_ms) >= self.safety.no_progress_ms {
                let _ = self.motor_stop();
                return Ok(DosingStatus::Aborted(DoserError::State(
                    "no progress".into(),
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
        self.last_weight_cg = 0;
        self.motor_started = false;
        self.last_progress_cg = 0;
        self.last_progress_at_ms = now;
        self.estop_latched = false;
        self.estop_count = 0;
    }

    /// Stop the motor (best-effort).
    pub fn motor_stop(&mut self) -> Result<()> {
        self.motor
            .stop()
            .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
            .wrap_err("motor_stop")
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
            let mid = self.tmp_med_buf.len() / 2;
            if self.tmp_med_buf.len() % 2 == 0 {
                (self.tmp_med_buf[mid - 1] + self.tmp_med_buf[mid]) / 2
            } else {
                self.tmp_med_buf[mid]
            }
        } else {
            w_cg
        };

        // Moving average over the median output (rounded to nearest)
        if ma_win > 1 {
            self.ma_buf.push_back(after_median);
            if self.ma_buf.len() > ma_win {
                self.ma_buf.pop_front();
            }
            let sum: i32 = self.ma_buf.iter().copied().sum();
            let len = self.ma_buf.len() as i32;
            if sum >= 0 {
                (sum + (len / 2)) / len
            } else {
                (sum - (len / 2)) / len
            }
        } else {
            after_median
        }
    }

    /// One iteration of the dosing loop
    pub fn step(&mut self) -> Result<DosingStatus> {
        // If previously estopped, keep aborting until reset via begin()
        if self.estop_latched || self.poll_estop() {
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State("estop".into())));
        }

        // 1) read current weight
        let timeout = Duration::from_millis(self.timeouts.sensor_ms);
        let raw = self
            .scale
            .read(timeout)
            .map_err(|e| eyre::Report::new(map_hw_error_dyn(&*e)))
            .wrap_err("reading scale")?;

        // Apply calibration (raw counts -> grams -> centigrams) and filtering
        let w_cg_raw = ((self.calibration.to_grams(raw) * 100.0).round()) as i32;
        let w_cg = self.apply_filter(w_cg_raw);

        self.last_weight_cg = w_cg;
        let err_cg = self.target_cg - w_cg;
        let abs_err_cg = err_cg.unsigned_abs() as i32;

        let now = self.clock.ms_since(self.epoch);

        // 1a) hard runtime cap (>= to allow deterministic tests with 0ms)
        if now.saturating_sub(self.start_ms) >= self.safety.max_run_ms {
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State(
                "max run time exceeded".into(),
            )));
        }

        // 1b) excessive overshoot guard
        if w_cg > self.target_cg + self.max_overshoot_cg {
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State(
                "max overshoot exceeded".into(),
            )));
        }

        // 2) Reached or exceeded target? Stop and settle (asymmetric completion)
        if w_cg + self.epsilon_cg >= self.target_cg {
            let _ = self.motor_stop();
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

        // 3) choose coarse or fine speed (use proportional taper with integer error)
        let target_speed = if abs_err_cg <= self.slow_at_cg && self.slow_at_cg > 0 {
            let ratio = (abs_err_cg as f32 / self.slow_at_cg as f32).clamp(0.0, 1.0);
            let min_frac = 0.2_f32; // floor at 20% of fine speed
            let frac = min_frac + (1.0 - min_frac) * ratio; // [min_frac, 1.0]
            ((self.control.fine_speed as f32 * frac).max(1.0)) as u32
        } else {
            self.control.coarse_speed
        };

        // 3a) no-progress watchdog (only while motor is commanded to run)
        if self.safety.no_progress_ms > 0 && self.no_progress_epsilon_cg > 0 && target_speed > 0 {
            if (w_cg - self.last_progress_cg).unsigned_abs() as i32 >= self.no_progress_epsilon_cg {
                self.last_progress_cg = w_cg;
                self.last_progress_at_ms = now;
            } else if now.saturating_sub(self.last_progress_at_ms) >= self.safety.no_progress_ms {
                let _ = self.motor_stop();
                return Ok(DosingStatus::Aborted(DoserError::State(
                    "no progress".into(),
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
            _s: PhantomData,
            _m: PhantomData,
            _t: PhantomData,
        }
    }
}

impl<S, M, T> DoserBuilder<S, M, T> {
    /// Fallible build available in any type-state; returns detailed BuildError for missing pieces.
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

        if !(0.1..=5000.0).contains(&target_g) {
            return Err(eyre::Report::new(BuildError::InvalidConfig(
                "target grams out of range",
            )));
        }

        let filter = self.filter.unwrap_or_default();
        let control = self.control.unwrap_or_default();
        let safety = self.safety.unwrap_or_default();
        let timeouts = self.timeouts.unwrap_or_default();
        let calibration = self.calibration.unwrap_or_default();
        let clock: Arc<dyn Clock + Send + Sync> = match self.clock {
            Some(b) => Arc::from(b),
            None => Arc::new(MonotonicClock::new()),
        };
        let estop_debounce_n = self.estop_debounce_n.unwrap_or(2);

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

        // Capture capacities before moving filter
        let ma_cap = filter.ma_window.max(1);
        let med_cap = filter.median_window.max(1);

        // Establish epoch for monotonic timing
        let epoch = clock.now();
        let now = clock.ms_since(epoch); // 0

        // Precompute loop period (us)
        let period_us = 1_000_000u64 / (filter.sample_rate_hz as u64);

        // Precompute integer thresholds in centigrams
        let to_cg = |g: f32| ((g * 100.0).round()) as i32;
        let target_cg = to_cg(target_g);
        let epsilon_cg = to_cg(control.epsilon_g);
        let max_overshoot_cg = to_cg(safety.max_overshoot_g);
        let no_progress_epsilon_cg = to_cg(safety.no_progress_epsilon_g);
        let slow_at_cg = to_cg(control.slow_at_g);

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
                period_us,
                slow_at_cg,
                epsilon_cg,
                max_overshoot_cg,
                no_progress_epsilon_cg,
                motor_started: false,
                estop_check: self.estop_check,
                last_progress_cg: 0,
                last_progress_at_ms: now,
                estop_latched: false,
                estop_debounce_n,
                estop_count: 0,
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
    let period_us = 1_000_000u64 / (filter.sample_rate_hz as u64);
    let to_cg = |g: f32| ((g * 100.0).round()) as i32;
    let target_cg = to_cg(target_g);
    let epsilon_cg = to_cg(control.epsilon_g);
    let max_overshoot_cg = to_cg(safety.max_overshoot_g);
    let no_progress_epsilon_cg = to_cg(safety.no_progress_epsilon_g);
    let slow_at_cg = to_cg(control.slow_at_g);

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
        period_us,
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
    })
}

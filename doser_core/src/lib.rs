//! Core dosing logic (hardware-agnostic).
//! - Keeps all hardware behind doser_traits::Scale/Motor
//! - Exposes a small builder to wire config + traits
//! - One-step control loop (call step() from the CLI or a strategy)

pub mod error;

use crate::error::{DoserError, Result};
use std::collections::VecDeque;
use std::time::Duration;

/// Testable clock abstraction returning monotonic milliseconds.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// System clock implementation.
#[derive(Default)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
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
}
impl Default for Calibration {
    fn default() -> Self {
        Self {
            gain_g_per_count: 1.0,
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
}

impl Default for ControlCfg {
    fn default() -> Self {
        Self {
            slow_at_g: 1.0,
            hysteresis_g: 0.05,
            stable_ms: 250,
            coarse_speed: 1200,
            fine_speed: 250,
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

/// Main core object — owns the Scale/Motor and executes the control loop.
pub struct Doser {
    scale: Box<dyn doser_traits::Scale>,
    motor: Box<dyn doser_traits::Motor>,
    filter: FilterCfg,
    control: ControlCfg,
    safety: SafetyCfg,
    timeouts: Timeouts,
    calibration: Calibration,
    target_g: f32,
    // Clock for deterministic time in tests
    clock: Box<dyn Clock>,

    last_weight_g: f32,
    settled_since_ms: Option<u64>,
    // Track when the dosing run started to enforce a max runtime
    start_ms: u64,
    // Buffers for filtering
    ma_buf: VecDeque<f32>,
    med_buf: VecDeque<f32>,
    // Motor lifecycle
    motor_started: bool,
    // Optional E-stop callback; if returns true, abort immediately
    estop_check: Option<Box<dyn Fn() -> bool>>,
    // No-progress watchdog
    last_progress_w: f32,
    last_progress_at_ms: u64,
}

impl Doser {
    /// Start building a Doser.
    pub fn builder() -> DoserBuilder {
        DoserBuilder::default()
    }

    /// Return the last observed weight in grams.
    pub fn last_weight(&self) -> f32 {
        self.last_weight_g
    }

    /// Optionally set the tare baseline in raw counts.
    pub fn set_tare_counts(&mut self, zero_counts: i32) {
        self.calibration.zero_counts = zero_counts;
    }

    /// Return the configured filter parameters (currently informational).
    pub fn filter_cfg(&self) -> &FilterCfg {
        &self.filter
    }

    /// Reset per-run state (start time, settling window). Call before a new dose.
    pub fn begin(&mut self) {
        let now = self.clock.now_ms();
        self.start_ms = now;
        self.settled_since_ms = None;
        // Clear filter state for a fresh run
        self.ma_buf.clear();
        self.med_buf.clear();
        self.last_weight_g = 0.0;
        self.motor_started = false;
        self.last_progress_w = 0.0;
        self.last_progress_at_ms = now;
    }

    /// Stop the motor (best-effort).
    pub fn motor_stop(&mut self) -> Result<()> {
        self.motor
            .stop()
            .map_err(|e| DoserError::Hardware(e.to_string()))
    }

    fn apply_filter(&mut self, w: f32) -> f32 {
        // Ensure sane window sizes
        let med_win = self.filter.median_window.max(1);
        let ma_win = self.filter.ma_window.max(1);

        // Median prefilter over grams
        let after_median = if med_win > 1 {
            self.med_buf.push_back(w);
            if self.med_buf.len() > med_win {
                self.med_buf.pop_front();
            }
            let mut tmp: Vec<f32> = self.med_buf.iter().copied().collect();
            tmp.sort_by(|a, b| a.total_cmp(b));
            let mid = tmp.len() / 2;
            if tmp.len() % 2 == 0 {
                (tmp[mid - 1] + tmp[mid]) / 2.0
            } else {
                tmp[mid]
            }
        } else {
            w
        };

        // Moving average over the median output
        if ma_win > 1 {
            self.ma_buf.push_back(after_median);
            if self.ma_buf.len() > ma_win {
                self.ma_buf.pop_front();
            }
            let sum: f32 = self.ma_buf.iter().copied().sum();
            sum / (self.ma_buf.len() as f32)
        } else {
            after_median
        }
    }

    /// One iteration of the dosing loop:
    /// - reads the scale with a timeout
    /// - adjusts motor speed (coarse/fine)
    /// - tracks settling window inside the hysteresis band
    /// - returns `Running`, `Complete`, or `Aborted(err)`
    pub fn step(&mut self) -> Result<DosingStatus> {
        // E-stop check
        if let Some(check) = &self.estop_check {
            if check() {
                let _ = self.motor_stop();
                return Ok(DosingStatus::Aborted(DoserError::State("estop".into())));
            }
        }

        // 1) read current weight
        let timeout = Duration::from_millis(self.timeouts.sensor_ms);
        let raw = self.scale.read(timeout).map_err(|e| {
            let s = e.to_string();
            if s.to_lowercase().contains("timeout") {
                DoserError::Timeout
            } else {
                DoserError::Hardware(s)
            }
        })?;

        // Apply calibration (raw counts -> grams) and filtering
        let w_raw = self.calibration.to_grams(raw);
        let w = self.apply_filter(w_raw);

        self.last_weight_g = w;
        let err = self.target_g - w;
        let abs_err = err.abs();

        let now = self.clock.now_ms();

        // 1a) hard runtime cap (>= to allow deterministic tests with 0ms)
        if now.saturating_sub(self.start_ms) >= self.safety.max_run_ms {
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State(
                "max run time exceeded".into(),
            )));
        }

        // 1b) excessive overshoot guard
        if w > self.target_g + self.safety.max_overshoot_g {
            let _ = self.motor_stop();
            return Ok(DosingStatus::Aborted(DoserError::State(
                "max overshoot exceeded".into(),
            )));
        }

        // 1c) no-progress watchdog (disabled when thresholds are zero)
        if self.safety.no_progress_ms > 0 && self.safety.no_progress_epsilon_g > 0.0 {
            if (w - self.last_progress_w).abs() >= self.safety.no_progress_epsilon_g {
                self.last_progress_w = w;
                self.last_progress_at_ms = now;
            } else if now.saturating_sub(self.last_progress_at_ms) >= self.safety.no_progress_ms {
                let _ = self.motor_stop();
                return Ok(DosingStatus::Aborted(DoserError::State(
                    "no progress".into(),
                )));
            }
        }

        // 2) Reached or exceeded target? Stop and settle (asymmetric completion)
        if w >= self.target_g {
            let _ = self.motor_stop();
            if self.settled_since_ms.is_none() {
                self.settled_since_ms = Some(now);
            }
            let stable_for_ms = now - self.settled_since_ms.unwrap();
            if stable_for_ms >= self.control.stable_ms {
                return Ok(DosingStatus::Complete);
            }
            return Ok(DosingStatus::Running);
        } else {
            // Below target: not in completion zone; reset settle timer
            self.settled_since_ms = None;
        }

        // 3) choose coarse or fine speed
        let target_speed = if abs_err <= self.control.slow_at_g {
            self.control.fine_speed
        } else {
            self.control.coarse_speed
        };

        // Ensure motor is started before commanding speed
        if !self.motor_started {
            self.motor
                .start()
                .map_err(|e| DoserError::Hardware(e.to_string()))?;
            self.motor_started = true;
        }

        self.motor
            .set_speed(target_speed)
            .map_err(|e| DoserError::Hardware(e.to_string()))?;

        Ok(DosingStatus::Running)
    }
}

/// Builder for `Doser`. All fields are validated on `build()`.
#[derive(Default)]
pub struct DoserBuilder {
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
    // Optional clock for tests
    clock: Option<Box<dyn Clock>>,
}

impl DoserBuilder {
    /// Supply a Scale implementation.
    pub fn with_scale<S: doser_traits::Scale + 'static>(mut self, s: S) -> Self {
        self.scale = Some(Box::new(s));
        self
    }

    /// Supply a Motor implementation.
    pub fn with_motor<M: doser_traits::Motor + 'static>(mut self, m: M) -> Self {
        self.motor = Some(Box::new(m));
        self
    }

    /// Supply filter config (optional).
    pub fn with_filter(mut self, f: FilterCfg) -> Self {
        self.filter = Some(f);
        self
    }

    /// Supply control config (optional).
    pub fn with_control(mut self, c: ControlCfg) -> Self {
        self.control = Some(c);
        self
    }

    /// Supply safety config (optional).
    pub fn with_safety(mut self, s: SafetyCfg) -> Self {
        self.safety = Some(s);
        self
    }

    /// Supply timeouts (optional).
    pub fn with_timeouts(mut self, t: Timeouts) -> Self {
        self.timeouts = Some(t);
        self
    }

    /// Supply calibration parameters directly.
    pub fn with_calibration(mut self, cal: Calibration) -> Self {
        self.calibration = Some(cal);
        self
    }

    /// Convenience: set gain and offset in grams.
    pub fn with_calibration_gain_offset(mut self, gain_g_per_count: f32, offset_g: f32) -> Self {
        let mut cal = self.calibration.unwrap_or_default();
        cal.gain_g_per_count = gain_g_per_count;
        cal.offset_g = offset_g;
        self.calibration = Some(cal);
        self
    }

    /// Convenience: provide tare baseline in raw counts.
    pub fn with_tare_counts(mut self, zero_counts: i32) -> Self {
        let mut cal = self.calibration.unwrap_or_default();
        cal.zero_counts = zero_counts;
        self.calibration = Some(cal);
        self
    }

    /// Optional no-op hook so the CLI can pass a calibration value without
    /// forcing a dependency on any config crate here.
    pub fn apply_calibration<T>(mut self, _calib: Option<&T>) -> Self {
        self._calibration_loaded = _calib.is_some();
        self
    }

    /// Set the target grams.
    pub fn with_target_grams(mut self, g: f32) -> Self {
        self.target_g = Some(g);
        self
    }

    /// Optional E-stop checker; return true to abort immediately.
    pub fn with_estop_check<F: Fn() -> bool + 'static>(mut self, f: F) -> Self {
        self.estop_check = Some(Box::new(f));
        self
    }

    /// Inject a custom clock (tests).
    pub fn with_clock<C: Clock + 'static>(mut self, c: C) -> Self {
        self.clock = Some(Box::new(c));
        self
    }

    /// Validate and build the Doser.
    pub fn build(self) -> Result<Doser> {
        let scale = self
            .scale
            .ok_or_else(|| DoserError::Config("scale not provided".into()))?;
        let motor = self
            .motor
            .ok_or_else(|| DoserError::Config("motor not provided".into()))?;
        let target_g = self
            .target_g
            .ok_or_else(|| DoserError::Config("target grams not provided".into()))?;

        if !(0.1..=5000.0).contains(&target_g) {
            return Err(DoserError::Config(format!(
                "target grams out of range: {target_g}"
            )));
        }

        let filter = self.filter.unwrap_or_default();
        let control = self.control.unwrap_or_default();
        let safety = self.safety.unwrap_or_default();
        let timeouts = self.timeouts.unwrap_or_default();
        let calibration = self.calibration.unwrap_or_default();
        let clock: Box<dyn Clock> = self.clock.unwrap_or_else(|| Box::new(SystemClock));

        // Capture capacities before moving filter
        let ma_cap = filter.ma_window.max(1);
        let med_cap = filter.median_window.max(1);

        let now = clock.now_ms();

        Ok(Doser {
            scale,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            clock,
            last_weight_g: 0.0,
            settled_since_ms: None,
            start_ms: now,
            ma_buf: VecDeque::with_capacity(ma_cap),
            med_buf: VecDeque::with_capacity(med_cap),
            motor_started: false,
            estop_check: self.estop_check,
            last_progress_w: 0.0,
            last_progress_at_ms: now,
        })
    }
}

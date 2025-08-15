//! Core dosing logic (hardware-agnostic).
//! - Keeps all hardware behind doser_traits::Scale/Motor
//! - Exposes a small builder to wire config + traits
//! - One-step control loop (call step() from the CLI or a strategy)

pub mod error;

use crate::error::{DoserError, Result};
use std::time::{Duration, Instant};

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
    #[allow(dead_code)] // Informational for now; reserved for future filtering logic
    filter: FilterCfg,
    control: ControlCfg,
    timeouts: Timeouts,
    target_g: f32,

    last_weight_g: f32,
    settled_since: Option<Instant>,
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

    /// Return the configured filter parameters (currently informational).
    pub fn filter_cfg(&self) -> &FilterCfg {
        &self.filter
    }

    /// Stop the motor (best-effort).
    pub fn motor_stop(&mut self) -> Result<()> {
        self.motor
            .stop()
            .map_err(|e| DoserError::Hardware(e.to_string()))
    }

    /// One iteration of the dosing loop:
    /// - reads the scale with a timeout
    /// - adjusts motor speed (coarse/fine)
    /// - tracks settling window inside the hysteresis band
    /// - returns `Running`, `Complete`, or `Aborted(err)`
    pub fn step(&mut self) -> Result<DosingStatus> {
        // 1) read current weight
        let timeout = Duration::from_millis(self.timeouts.sensor_ms);
        let raw = self
            .scale
            .read(timeout)
            .map_err(|e| DoserError::Hardware(e.to_string()))?;

        // If your Scale returns raw ADC units, apply calibration here.
        // For this minimal core, assume upstream returns grams (i32 -> f32).
        let w = raw as f32;

        self.last_weight_g = w;
        let err = self.target_g - w;
        let abs_err = err.abs();

        // 2) inside hysteresis band? attempt to settle
        if abs_err <= self.control.hysteresis_g {
            if self.settled_since.is_none() {
                self.settled_since = Some(Instant::now());
            }
            let stable_for = self.settled_since.unwrap().elapsed();
            if stable_for.as_millis() as u64 >= self.control.stable_ms {
                let _ = self.motor_stop();
                return Ok(DosingStatus::Complete);
            }
            return Ok(DosingStatus::Running);
        } else {
            // left the band; reset settling timer
            self.settled_since = None;
        }

        // 3) choose coarse or fine speed
        let target_speed = if abs_err <= self.control.slow_at_g {
            self.control.fine_speed
        } else {
            self.control.coarse_speed
        };

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
    timeouts: Option<Timeouts>,
    target_g: Option<f32>,
    // keep for future use; no-op in this minimal core
    _calibration_loaded: bool,
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

    /// Supply timeouts (optional).
    pub fn with_timeouts(mut self, t: Timeouts) -> Self {
        self.timeouts = Some(t);
        self
    }

    /// Set the target grams.
    pub fn with_target_grams(mut self, g: f32) -> Self {
        self.target_g = Some(g);
        self
    }

    /// Optional no-op hook so the CLI can pass a calibration value without
    /// forcing a dependency on any config crate here.
    pub fn apply_calibration<T>(mut self, _calib: Option<&T>) -> Self {
        self._calibration_loaded = _calib.is_some();
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
        let timeouts = self.timeouts.unwrap_or_default();

        Ok(Doser {
            scale,
            motor,
            filter,
            control,
            timeouts,
            target_g,
            last_weight_g: 0.0,
            settled_since: None,
        })
    }
}

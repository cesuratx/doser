//! Configuration types for the dosing engine.
//!
//! These are the runtime configuration structs used by `DoserCore`.
//! They are separate from the TOML-deserialized config in `doser_config`.

/// Filter configuration for signal conditioning.
#[derive(Debug, Clone)]
pub struct FilterCfg {
    /// Moving average window size (1 = disabled).
    pub ma_window: usize,
    /// Median prefilter window size (1 = disabled).
    pub median_window: usize,
    /// Sampling rate in Hz (informational; drives loop period).
    pub sample_rate_hz: u32,
    /// EMA smoothing factor; when > 0, EMA is used instead of moving average.
    /// Range: (0.0, 1.0]. 0.0 disables EMA and uses moving average when `ma_window > 1`.
    pub ema_alpha: f32,
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

/// Filter selection for the smoothing stage (after optional median).
/// Informational; the active variant is derived from `FilterCfg`.
#[derive(Debug, Clone, Copy)]
pub enum FilterKind {
    MovingAverage { window: usize },
    Median { window: usize },
    Ema { alpha: f32 },
}

/// Control configuration (speed management, settling).
#[derive(Debug, Clone)]
pub struct ControlCfg {
    /// Speed table: each entry is `(threshold_g, sps)`. Sorted descending by threshold at build.
    /// When non-empty, takes precedence over two-speed mode.
    pub speed_bands: Vec<(f32, u32)>,
    /// Switch to fine speed once `err <= slow_at_g` (used when `speed_bands.is_empty()`).
    pub slow_at_g: f32,
    /// Consider "in band" if `|err| <= hysteresis_g`. Default: 0.07 g.
    pub hysteresis_g: f32,
    /// Weight must stay within hysteresis for this many ms to be "settled".
    pub stable_ms: u64,
    /// Coarse motor speed (steps per second).
    pub coarse_speed: u32,
    /// Fine motor speed for the final approach.
    pub fine_speed: u32,
    /// Tolerance below target (grams) to enter completion zone. Default: 0.08 g.
    pub epsilon_g: f32,
}

impl Default for ControlCfg {
    fn default() -> Self {
        Self {
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
    /// Set to 0.0 to disable.
    pub no_progress_epsilon_g: f32,
    /// See `no_progress_epsilon_g`. 0 disables the watchdog.
    pub no_progress_ms: u64,
}

impl Default for SafetyCfg {
    fn default() -> Self {
        Self {
            max_run_ms: 60_000,
            max_overshoot_g: 2.0,
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        }
    }
}

/// Timeouts and watchdogs.
#[derive(Debug, Clone)]
pub struct Timeouts {
    /// Max sensor wait per read (ms).
    pub sensor_ms: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { sensor_ms: 150 }
    }
}

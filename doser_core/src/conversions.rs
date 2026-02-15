//! `From` implementations bridging `doser_config` types to `doser_core` types.
//!
//! These eliminate the manual field-by-field mapping previously scattered in the CLI.

use crate::calibration::Calibration;
use crate::config::{ControlCfg, FilterCfg, PredictorCfg, SafetyCfg, Timeouts};

// ── FilterCfg ────────────────────────────────────────────────────────────────

impl From<&doser_config::FilterCfg> for FilterCfg {
    fn from(c: &doser_config::FilterCfg) -> Self {
        Self {
            ma_window: c.ma_window,
            median_window: c.median_window,
            sample_rate_hz: c.sample_rate_hz,
            ema_alpha: c.ema_alpha.unwrap_or(0.0),
        }
    }
}

// ── ControlCfg ───────────────────────────────────────────────────────────────

impl From<&doser_config::ControlCfg> for ControlCfg {
    fn from(c: &doser_config::ControlCfg) -> Self {
        Self {
            speed_bands: c.speed_bands.clone(),
            coarse_speed: c.coarse_speed,
            fine_speed: c.fine_speed,
            slow_at_g: c.slow_at_g,
            hysteresis_g: c.hysteresis_g,
            stable_ms: c.stable_ms,
            epsilon_g: c.epsilon_g,
        }
    }
}

// ── SafetyCfg ────────────────────────────────────────────────────────────────

impl From<&doser_config::Safety> for SafetyCfg {
    fn from(c: &doser_config::Safety) -> Self {
        Self {
            max_run_ms: c.max_run_ms,
            max_overshoot_g: c.max_overshoot_g,
            no_progress_epsilon_g: c.no_progress_epsilon_g,
            no_progress_ms: c.no_progress_ms,
        }
    }
}

// ── Timeouts ─────────────────────────────────────────────────────────────────

impl From<&doser_config::Timeouts> for Timeouts {
    fn from(c: &doser_config::Timeouts) -> Self {
        Self {
            sensor_ms: c.sample_ms,
        }
    }
}

// ── PredictorCfg ─────────────────────────────────────────────────────────────

impl From<&doser_config::PredictorCfg> for PredictorCfg {
    fn from(c: &doser_config::PredictorCfg) -> Self {
        Self {
            enabled: c.enabled,
            window: c.window,
            extra_latency_ms: c.extra_latency_ms,
            min_progress_ratio: c.min_progress_ratio,
        }
    }
}

// ── Calibration ──────────────────────────────────────────────────────────────

impl From<&doser_config::Calibration> for Calibration {
    fn from(c: &doser_config::Calibration) -> Self {
        Self {
            gain_g_per_count: c.scale_factor,
            zero_counts: c.offset,
            offset_g: 0.0,
        }
    }
}

impl From<&doser_config::PersistedCalibration> for Calibration {
    fn from(c: &doser_config::PersistedCalibration) -> Self {
        Self {
            gain_g_per_count: c.gain_g_per_count,
            zero_counts: c.zero_counts,
            offset_g: c.offset_g,
        }
    }
}

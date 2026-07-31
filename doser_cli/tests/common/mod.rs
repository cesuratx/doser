//! Shared fixtures for the `doser_cli` integration tests.
//!
//! `doser_cli` is a binary-only crate, so nothing here can call into its
//! modules — every test drives the built binary and asserts on its observable
//! contract (exit code, stdout, stderr). This module only builds the inputs.

// Each integration-test binary uses a different subset of these helpers.
#![allow(dead_code)]

use assert_cmd::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The knobs the tests need to vary. Everything else in the emitted TOML is a
/// fixed, validation-clean baseline, so a test only states what it exercises.
pub struct Cfg {
    pub sample_rate_hz: u32,
    pub sample_ms: u64,
    pub sensor_read_timeout_ms: u64,
    pub max_run_ms: u64,
    pub max_overshoot_g: f32,
    pub no_progress_ms: u64,
    pub no_progress_epsilon_g: f32,
    pub epsilon_g: f32,
    pub coarse_speed: u32,
    /// Extra TOML appended verbatim (e.g. a `[calibration]` table).
    pub extra: String,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            // 50 Hz with a 20 ms per-read timeout keeps the stall watchdog
            // (4x sensor timeout = 80 ms) well clear of the sim's read latency,
            // so only the watchdog a test is actually aiming at can fire.
            sample_rate_hz: 50,
            sample_ms: 20,
            sensor_read_timeout_ms: 100,
            max_run_ms: 4000,
            max_overshoot_g: 5.0,
            no_progress_ms: 1200,
            no_progress_epsilon_g: 0.02,
            epsilon_g: 0.02,
            coarse_speed: 1000,
            extra: String::new(),
        }
    }
}

impl Cfg {
    pub fn to_toml(&self) -> String {
        let Self {
            sample_rate_hz,
            sample_ms,
            sensor_read_timeout_ms,
            max_run_ms,
            max_overshoot_g,
            no_progress_ms,
            no_progress_epsilon_g,
            epsilon_g,
            coarse_speed,
            extra,
        } = self;
        format!(
            "\
[pins]
# pins are unused by the sim backend but the schema requires them
hx711_dt = 5
hx711_sck = 6
motor_step = 13
motor_dir = 19
motor_en = 26
estop_in = 21

[filter]
ma_window = 1
median_window = 1
sample_rate_hz = {sample_rate_hz}

[control]
coarse_speed = {coarse_speed}
fine_speed = 200
slow_at_g = 1.0
hysteresis_g = 0.05
stable_ms = 0
epsilon_g = {epsilon_g}

[timeouts]
sample_ms = {sample_ms}

[safety]
max_run_ms = {max_run_ms}
max_overshoot_g = {max_overshoot_g}
no_progress_epsilon_g = {no_progress_epsilon_g}
no_progress_ms = {no_progress_ms}

[hardware]
sensor_read_timeout_ms = {sensor_read_timeout_ms}
{extra}"
        )
    }

    /// Write the config into `dir` and return its path.
    pub fn write(&self, dir: &tempfile::TempDir) -> PathBuf {
        write_named(dir, "cfg.toml", &self.to_toml())
    }
}

/// Write the baseline config into `dir` and return its path.
pub fn write_valid_config(dir: &tempfile::TempDir) -> PathBuf {
    Cfg::default().write(dir)
}

/// Write arbitrary `contents` to `dir/name` and return the path.
pub fn write_named(dir: &tempfile::TempDir, name: &str, contents: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).expect("write test fixture");
    path
}

/// A `doser_cli` invocation pointed at `cfg`. Callers add the subcommand.
pub fn doser(cfg: &Path) -> Command {
    let mut cmd = Command::cargo_bin("doser_cli").expect("doser_cli test binary is built");
    cmd.arg("--config").arg(cfg);
    cmd
}

/// The exit codes `error_fmt::exit_code_for_error` promises. Kept here so every
/// test names the same constant and a contract change breaks one obvious place.
pub mod exit {
    /// Any non-abort failure (config, calibration, scale timeout, ...).
    pub const OTHER: i32 = 1;
    /// `AbortReason::Estop`. NOTE: clap's own usage error is also 2, so an
    /// operator script cannot distinguish "E-stop tripped" from "you typed the
    /// command wrong" by exit code alone. That collision is a real wart in the
    /// contract; it is recorded here rather than silently worked around.
    pub const ESTOP: i32 = 2;
    pub const NO_PROGRESS: i32 = 3;
    pub const MAX_RUNTIME: i32 = 4;
    pub const OVERSHOOT: i32 = 5;
    pub const MAX_ATTEMPTS: i32 = 6;
    /// clap's usage error (missing/invalid argument). Same number as `ESTOP`.
    pub const USAGE: i32 = 2;
}

//! CLI argument definitions and shared statics.

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::sync::OnceLock;

pub static FILE_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();
/// Whether the user asked for JSON output (controls structured error output).
pub static JSON_MODE: OnceLock<bool> = OnceLock::new();
/// Effective safety knobs used for the current run (for JSON details).
pub static LAST_SAFETY: OnceLock<CliSafety> = OnceLock::new();

#[derive(Copy, Clone, Debug)]
pub struct CliSafety {
    pub max_run_ms: u64,
    pub max_overshoot_g: f32,
    pub no_progress_ms: u64,
    pub no_progress_epsilon_g: f32,
}

#[derive(Clone, Copy, Default)]
pub struct JsonTelemetry {
    pub slope_ema_gps: Option<f32>,
    pub stop_at_g: Option<f32>,
    pub coast_comp_g: Option<f32>,
}

#[derive(Parser, Debug)]
#[command(name = "doser", version, about = "Doser CLI")]
pub struct Cli {
    /// Path to config TOML (typed)
    #[arg(long, value_name = "FILE", default_value = "etc/doser_config.toml")]
    pub config: PathBuf,

    /// Optional calibration CSV (strict header)
    #[arg(long, value_name = "FILE")]
    pub calibration: Option<PathBuf>,

    /// Log as JSON lines instead of pretty
    #[arg(long, action = ArgAction::SetTrue)]
    pub json: bool,

    /// Console log level (error|warn|info|debug|trace)
    #[arg(long = "log-level", value_name = "LEVEL", default_value = "info")]
    pub log_level: String,

    /// Command to execute
    #[command(subcommand)]
    pub cmd: Commands,
}

/// Memory locking mode for real-time operation.
#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum RtLock {
    /// Do not lock memory
    None,
    /// Lock currently resident pages
    Current,
    /// Lock current and future pages
    All,
}

impl RtLock {
    #[inline]
    pub fn os_default() -> Self {
        #[cfg(target_os = "linux")]
        {
            return RtLock::Current;
        }
        #[cfg(target_os = "macos")]
        {
            return RtLock::None;
        }
        #[allow(unreachable_code)]
        RtLock::None
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Dispense a target amount of material
    Dose {
        /// Target grams to dispense
        #[arg(long)]
        grams: f32,
        /// Override safety: max run time in ms (takes precedence over config)
        #[arg(long, value_name = "MS")]
        max_run_ms: Option<u64>,
        /// Override safety: abort if overshoot exceeds this many grams
        #[arg(long, value_name = "GRAMS")]
        max_overshoot_g: Option<f32>,
        /// Use direct control loop (no sampler); reads the scale inside the control loop
        #[arg(long, action = ArgAction::SetTrue)]
        direct: bool,
        /// Print total runtime on completion
        #[arg(long, action = ArgAction::SetTrue)]
        print_runtime: bool,
        /// Enable real-time mode (SCHED_FIFO, affinity, mlockall)
        #[arg(
            long,
            action = ArgAction::SetTrue,
            long_help = "Enable real-time mode on supported OSes.\n\nLinux: Attempts SCHED_FIFO priority, pins to CPU 0, and calls mlockall(MCL_CURRENT|MCL_FUTURE) to lock the process address space into RAM. This reduces page faults and jitter but can impact overall system performance and may require elevated privileges or ulimits (e.g., memlock). Use with care on shared systems.\n\nmacOS: Only mlockall is applied; SCHED_FIFO/affinity are unavailable. Locking memory can increase pressure on the OS memory manager."
        )]
        rt: bool,
        /// Real-time priority for SCHED_FIFO on Linux (1..=max); ignored on macOS
        #[arg(
            long,
            value_name = "PRIO",
            long_help = "SCHED_FIFO priority when --rt is enabled (Linux only). Higher values run before lower ones. Range is platform-defined (usually 1..=99). Use with care; very high priorities can impact system stability."
        )]
        rt_prio: Option<i32>,
        /// Select memory locking mode for --rt: none, current, or all
        #[arg(
            long,
            value_enum,
            value_name = "MODE",
            long_help = "Select memory locking mode when --rt is enabled.\n- none: do not lock memory.\n- current: lock currently resident pages (mlockall(MCL_CURRENT)).\n- all: lock current and future pages (mlockall(MCL_CURRENT|MCL_FUTURE)).\nDefault: current on Linux, none on macOS."
        )]
        rt_lock: Option<RtLock>,
        /// Real-time CPU index to pin the process to (Linux only). If not set, defaults to 0.
        #[arg(
            long,
            value_name = "CPU",
            long_help = "Select the CPU index to pin the process to when --rt is enabled (Linux only). Defaults to 0. The value must be allowed by the current affinity mask; otherwise affinity will be left unchanged and a warning is logged."
        )]
        rt_cpu: Option<usize>,
        /// Print control loop and sampling stats
        #[arg(long, action = ArgAction::SetTrue)]
        stats: bool,
    },
    /// Quick health check (hardware presence / sim ok)
    SelfCheck,
    /// Health check for operational monitoring
    Health,
}

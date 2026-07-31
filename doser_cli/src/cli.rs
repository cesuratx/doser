//! CLI argument definitions and shared statics.

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use doser_hardware::MAX_STEP_RATE_SPS;
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
    #[must_use]
    pub const fn os_default() -> Self {
        #[cfg(target_os = "linux")]
        {
            return Self::Current;
        }
        #[cfg(target_os = "macos")]
        {
            return Self::None;
        }
        #[allow(unreachable_code)]
        Self::None
    }
}

// `doc_markdown` allowed: clap renders these doc comments verbatim as `--help` text,
// so backticks added for rustdoc's benefit would show up in the terminal output.
#[allow(clippy::doc_markdown)]
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
    /// Serve a live web UI showing the current scale reading (for dev/testing)
    Monitor {
        /// TCP port to listen on
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// Address to bind; the default 0.0.0.0 exposes an UNAUTHENTICATED UI to the whole LAN
        #[arg(
            long,
            default_value = "0.0.0.0",
            long_help = "Address to bind the monitor's HTTP server to.\n\nThe default 0.0.0.0 listens on every interface, which is what makes the UI reachable from another machine on the LAN — but the server has NO authentication and NO transport security, so anyone who can reach the Pi can read the live scale feed. Use 127.0.0.1 to keep it on the Pi itself, and only bind a routable address on a network you trust."
        )]
        bind: String,
        /// Override sample rate in Hz (defaults to config `filter.sample_rate_hz`)
        #[arg(long, value_name = "HZ")]
        hz: Option<u32>,
    },
    /// Jog the motor at a fixed rate for bring-up/testing (no scale, no control loop)
    Motor {
        // Upper bound is `doser_hardware::MAX_STEP_RATE_SPS` (5000) — both motor
        // backends clamp to it, so rejecting out-of-range rates here keeps the
        // commanded rate and the stepped rate the same number.
        /// Step rate in steps-per-second (1..=5000, the driver's max)
        #[arg(
            long,
            value_name = "HZ",
            default_value_t = 200,
            value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_STEP_RATE_SPS)),
        )]
        sps: u32,
        /// How long to run, in milliseconds
        #[arg(long, value_name = "MS", default_value_t = 1000)]
        ms: u64,
        /// Approximate step count instead of a duration (overrides --ms)
        #[arg(
            long,
            value_name = "N",
            long_help = "Run for roughly N steps instead of --ms (overrides it).\n\nThe count is APPROXIMATE, not exact: it is turned into a wall-clock duration of N/--sps seconds (rounded up) and the stepping thread paces the pulses, so scheduling jitter and the rounding can land a step or two either side of N. Use it to move a known-ish amount during bring-up, not to index a mechanism precisely."
        )]
        steps: Option<u32>,
        /// Rotation direction
        #[arg(long, value_enum, default_value_t = Direction::Cw)]
        dir: Direction,
    },
}

/// Motor rotation direction for the `motor` jog command.
#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum Direction {
    /// Clockwise (DIR line high)
    Cw,
    /// Counterclockwise (DIR line low)
    Ccw,
}

impl Direction {
    /// True when clockwise; maps directly to the DIR line level.
    #[inline]
    #[must_use]
    pub const fn is_clockwise(self) -> bool {
        matches!(self, Self::Cw)
    }
}

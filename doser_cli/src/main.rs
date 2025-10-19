#![cfg_attr(all(not(debug_assertions), not(test)), deny(warnings))]
#![cfg_attr(
    all(not(debug_assertions), not(test)),
    deny(clippy::all, clippy::pedantic, clippy::nursery)
)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]
//! CLI entrypoint for the dosing system.
//!
//! Responsibilities:
//! - Parse config/flags and assemble hardware and core components
//! - Initialize tracing and manage log sinks
//! - Offer `--json` mode emitting stable JSONL lines to stdout (logs to stderr)
//! - Provide optional RT helpers via libc on supported OSes, with safety docs
//! - Map domain abort reasons to stable exit codes
use std::{fs, path::PathBuf};

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use doser_config::{Calibration, Config, load_calibration_csv};
use doser_core::error::Result as CoreResult;
use eyre::WrapErr;

use std::sync::OnceLock;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use doser_core::runner::SamplingMode;

// Local NoopScale for sampler-driven mode (removed; orchestration moved to core runner)

static FILE_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();
// Remember whether the user asked for JSON output (controls structured error output)
static JSON_MODE: OnceLock<bool> = OnceLock::new();
// Remember effective safety knobs used for the current run (for JSON details)
#[derive(Copy, Clone, Debug)]
struct CliSafety {
    max_run_ms: u64,
    max_overshoot_g: f32,
    no_progress_ms: u64,
    no_progress_epsilon_g: f32,
}
static LAST_SAFETY: OnceLock<CliSafety> = OnceLock::new();

#[derive(Clone, Copy, Default)]
struct JsonTelemetry {
    slope_ema_gps: Option<f32>,
    stop_at_g: Option<f32>,
    coast_comp_g: Option<f32>,
}

fn humanize(err: &eyre::Report) -> String {
    use doser_core::error::{BuildError, DoserError};

    // Typed matches first
    if let Some(be) = err.downcast_ref::<BuildError>() {
        return match be {
            BuildError::MissingScale => {
                "What happened: No scale was provided to the dosing engine.\nLikely causes: Hardware scale failed to initialize or was not wired into the builder.\nHow to fix: Ensure the HX711 scale is created successfully and passed via with_scale(...).".to_string()
            }
            BuildError::MissingMotor => {
                "What happened: No motor was provided to the dosing engine.\nLikely causes: Motor driver failed to initialize or was not wired into the builder.\nHow to fix: Ensure the motor is created successfully and passed via with_motor(...).".to_string()
            }
            BuildError::MissingTarget => {
                "What happened: Target grams not set.\nLikely causes: The CLI did not pass --grams or the builder was not configured.\nHow to fix: Provide the desired grams (e.g., `doser dose --grams 10`).".to_string()
            }
            BuildError::InvalidConfig(msg) => format!(
                "What happened: Invalid configuration ({msg}).\nLikely causes: Missing or out-of-range values in the TOML.\nHow to fix: Edit the config file, then rerun. See README for a sample."
            ),
        };
    }

    if let Some(de) = err.downcast_ref::<DoserError>() {
        // Specific domain cases first
        if matches!(de, DoserError::Timeout) {
            return "What happened: Scale read timed out.\nLikely causes: HX711 not wired correctly, no power/ground, or timeout too low.\nHow to fix: Verify DT/SCK pins and power, and consider increasing hardware.sensor_read_timeout_ms in the config.".to_string();
        }
        if let DoserError::Abort(reason) = de {
            use doser_core::error::AbortReason::*;
            return match reason {
                Estop => "What happened: Emergency stop was triggered.\nLikely causes: E-stop button pressed or input pin active.\nHow to fix: Release E-stop, ensure wiring is correct, then start a new run.".to_string(),
                NoProgress => "What happened: No progress watchdog tripped.\nLikely causes: Jammed auger, empty hopper, or scale not changing within threshold.\nHow to fix: Check mechanics and materials; adjust safety.no_progress_* in config if needed.".to_string(),
                MaxRuntime => "max run time was exceeded.\nLikely causes: Too conservative speeds, high target, or stalls.\nHow to fix: Increase safety.max_run_ms or adjust speeds/target.".to_string(),
                Overshoot => "What happened: Overshoot beyond safety limit.\nLikely causes: Inertia or too high coarse/fine speed near target.\nHow to fix: Lower speeds or increase safety.max_overshoot_g and tune epsilon/slow_at.".to_string(),
                MaxAttempts => "What happened: Internal strategy aborted after maximum attempts.\nLikely causes: Conservative settings or unexpected stall in strategy loop.\nHow to fix: Increase attempts or review control/safety settings.".to_string(),
            };
        }
        // Fallback to generic for other domain errors
        return format!(
            "What happened: {de}.\nLikely causes: See logs.\nHow to fix: Re-run with --log-level=debug or set RUST_LOG for more detail."
        );
    }

    // String-based heuristics for errors coming from init or config
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();

    if (lower.contains("hx711") && lower.contains("timeout")) || lower.contains("datareadytimeout")
    {
        return "What happened: HX711 did not produce data within the configured timeout.\nLikely causes: Wrong DT/SCK pins, wiring/power issues, or timeout configured too low.\nHow to fix: Check [pins] in the config, verify 5V/GND, and raise hardware.sensor_read_timeout_ms.".to_string();
    }

    if lower.contains("open hx711") || lower.contains("open motor pins") {
        return "What happened: Failed to initialize hardware pins.\nLikely causes: Incorrect pin numbers or insufficient GPIO permissions.\nHow to fix: Fix the [pins] values in the config; ensure the process has permission to access GPIO.".to_string();
    }

    if lower.contains("invalid configuration")
        || (lower.contains("pin") && lower.contains("missing"))
    {
        return "What happened: Configuration is invalid or incomplete.\nLikely causes: Missing [pins] (hx711_dt, hx711_sck, motor_step, motor_dir, ...), or out-of-range values.\nHow to fix: Edit the TOML config and try again.".to_string();
    }

    // Calibration CSV header special-case
    if lower.contains("calibration csv must have headers") {
        return "Invalid headers in calibration CSV. Expected 'raw,grams'.".to_string();
    }

    // Generic fallback
    let mut cause = String::new();
    if let Some(src) = err.source() {
        cause = format!(" Cause: {src}");
    }
    format!(
        "Something went wrong.{cause}\nHow to fix: Re-run with --log-level=debug for details. Original: {msg}"
    )
}

/// Map AbortReason (if present) to stable exit codes; non-abort errors return 2.
fn exit_code_for_error(err: &eyre::Report) -> i32 {
    use doser_core::error::DoserError;
    if let Some(DoserError::Abort(reason)) = err.downcast_ref::<DoserError>() {
        return match reason {
            doser_core::error::AbortReason::Estop => 2,
            doser_core::error::AbortReason::NoProgress => 3,
            doser_core::error::AbortReason::MaxRuntime => 4,
            doser_core::error::AbortReason::Overshoot => 5,
            doser_core::error::AbortReason::MaxAttempts => 6,
        };
    }
    1
}

fn abort_reason_name(r: &doser_core::error::AbortReason) -> &'static str {
    use doser_core::error::AbortReason::*;
    match r {
        Estop => "Estop",
        NoProgress => "NoProgress",
        MaxRuntime => "MaxRuntime",
        Overshoot => "Overshoot",
        MaxAttempts => "MaxAttempts",
    }
}

/// Structured JSON for errors when --json is enabled.
fn format_error_json(err: &eyre::Report) -> String {
    use doser_core::error::DoserError;
    if let Some(DoserError::Abort(reason)) = err.downcast_ref::<DoserError>() {
        // Keep schema small and stable for scripts
        let msg = humanize(err).replace('"', "\\\"");
        let details = LAST_SAFETY.get();
        match reason {
            doser_core::error::AbortReason::Overshoot => {
                if let Some(s) = details {
                    return format!(
                        "{{\"reason\":\"{}\",\"details\":{{\"max_overshoot_g\":{}}},\"message\":\"{}\"}}",
                        abort_reason_name(reason),
                        s.max_overshoot_g,
                        msg
                    );
                }
            }
            doser_core::error::AbortReason::MaxRuntime => {
                if let Some(s) = details {
                    return format!(
                        "{{\"reason\":\"{}\",\"details\":{{\"max_run_ms\":{}}},\"message\":\"{}\"}}",
                        abort_reason_name(reason),
                        s.max_run_ms,
                        msg
                    );
                }
            }
            doser_core::error::AbortReason::NoProgress => {
                if let Some(s) = details {
                    return format!(
                        "{{\"reason\":\"{}\",\"details\":{{\"no_progress_ms\":{},\"epsilon_g\":{}}},\"message\":\"{}\"}}",
                        abort_reason_name(reason),
                        s.no_progress_ms,
                        s.no_progress_epsilon_g,
                        msg
                    );
                }
            }
            _ => {}
        }
        // Fallback without details
        return format!(
            "{{\"reason\":\"{}\",\"message\":\"{}\"}}",
            abort_reason_name(reason),
            msg
        );
    }
    // Generic error JSON
    format!(
        "{{\"reason\":\"Error\",\"message\":\"{}\"}}",
        humanize(err).replace('"', "\\\"")
    )
}

/// Build a file sink writer with optional rotation, storing the non-blocking guard in OnceLock.
fn file_layer(
    file: Option<&str>,
    rotation: Option<&str>,
) -> Option<tracing_appender::non_blocking::NonBlocking> {
    let path = file?;
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file_appender = match rotation.unwrap_or("never").to_ascii_lowercase().as_str() {
        "daily" => tracing_appender::rolling::daily(".", path),
        "hourly" => tracing_appender::rolling::hourly(".", path),
        _ => tracing_appender::rolling::never(".", path),
    };
    let (nb_writer, guard) = tracing_appender::non_blocking(file_appender);
    let _ = FILE_GUARD.set(guard);
    Some(nb_writer)
}

/// Initialize tracing once for the whole app.
fn init_tracing(json: bool, level: &str, file: Option<&str>, rotation: Option<&str>) {
    // Prefer RUST_LOG if set; otherwise use CLI level
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let registry = tracing_subscriber::registry().with(filter);

    if json {
        let console = fmt::layer().json().with_target(false);
        if let Some(nb_writer) = file_layer(file, rotation) {
            let file_l = fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(nb_writer);
            registry.with(console).with(file_l).init();
        } else {
            registry.with(console).init();
        }
    } else {
        let console = fmt::layer().pretty().with_target(false);
        if let Some(nb_writer) = file_layer(file, rotation) {
            let file_l = fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(nb_writer);
            registry.with(console).with(file_l).init();
        } else {
            registry.with(console).init();
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "doser", version, about = "Doser CLI")]
struct Cli {
    /// Path to config TOML (typed)
    #[arg(long, value_name = "FILE", default_value = "etc/doser_config.toml")]
    config: PathBuf,

    /// Optional calibration CSV (strict header)
    #[arg(long, value_name = "FILE")]
    calibration: Option<PathBuf>,

    /// Log as JSON lines instead of pretty
    #[arg(long, action = ArgAction::SetTrue)]
    json: bool,

    /// Console log level (error|warn|info|debug|trace)
    #[arg(long = "log-level", value_name = "LEVEL", default_value = "info")]
    log_level: String,

    /// Command to execute
    #[command(subcommand)]
    cmd: Commands,
}

/// Memory locking mode for real-time operation
#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum RtLock {
    /// Do not lock memory
    None,
    /// Lock currently resident pages
    Current,
    /// Lock current and future pages
    All,
}

impl RtLock {
    #[inline]
    fn os_default() -> Self {
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
enum Commands {
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

fn main() -> eyre::Result<()> {
    // Initialize pretty error reports early
    let _ = color_eyre::install();

    // Set up graceful shutdown handler
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_clone = std::sync::Arc::clone(&shutdown);

    if let Err(e) = ctrlc::set_handler(move || {
        eprintln!("\nReceived shutdown signal, stopping gracefully...");
        shutdown_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }) {
        eprintln!("Warning: Failed to set signal handler: {e}");
    }

    if let Err(e) = real_main(shutdown) {
        let json = *JSON_MODE.get().unwrap_or(&false);
        let code = exit_code_for_error(&e);
        if json {
            // Emit a one-line JSON object for scripts
            println!("{}", format_error_json(&e));
        } else {
            eprintln!("{}", humanize(&e));
        }
        std::process::exit(code);
    }
    Ok(())
}

fn real_main(shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>) -> eyre::Result<()> {
    let cli = Cli::parse();
    let _ = JSON_MODE.set(cli.json);

    // 1) Load typed config from TOML (for logging.file)
    let cfg_text = fs::read_to_string(&cli.config)
        .wrap_err_with(|| format!("read config {:?}", cli.config))?;
    let cfg: Config =
        toml::from_str(&cfg_text).wrap_err_with(|| format!("parse config {:?}", cli.config))?;

    // Validate configuration with clear errors
    cfg.validate().wrap_err("invalid configuration")?;

    init_tracing(
        cli.json,
        &cli.log_level,
        cfg.logging.file.as_deref(),
        cfg.logging.rotation.as_deref(),
    );

    // 2) Load calibration: prefer persisted in TOML if present; else optional CSV
    let calib: Option<Calibration> = if let Some(pc) = cfg.calibration {
        Some(Calibration {
            offset: pc.zero_counts,
            scale_factor: pc.gain_g_per_count,
        })
    } else if let Some(p) = &cli.calibration {
        let c =
            load_calibration_csv(p).map_err(|e| eyre::eyre!("parse calibration {:?}: {}", p, e))?;
        Some(c)
    } else {
        None
    };

    // 3) Build hardware (feature-gated) or sim
    #[cfg(all(feature = "hardware", target_os = "linux"))]
    let hw = {
        use doser_hardware::{HardwareMotor, HardwareScale};
        let scale = HardwareScale::try_new_with_timeout(
            cfg.pins.hx711_dt,
            cfg.pins.hx711_sck,
            cfg.hardware.sensor_read_timeout_ms,
        )
        .wrap_err("open HX711")?;
        let motor = HardwareMotor::try_new_with_en(
            cfg.pins.motor_step,
            cfg.pins.motor_dir,
            cfg.pins.motor_en,
        )
        .wrap_err("open motor pins")?;
        (scale, motor)
    };

    #[cfg(any(not(feature = "hardware"), not(target_os = "linux")))]
    let hw = {
        use doser_hardware::{SimulatedMotor, SimulatedScale};
        (SimulatedScale::new(), SimulatedMotor::default())
    };

    match cli.cmd {
        Commands::SelfCheck => {
            tracing::info!("self-check starting");
            use doser_traits::Scale;
            use std::time::{Duration, Instant};

            // Move hw so we can probe it here; the process exits after SelfCheck.
            let (mut scale, _motor) = hw;

            // Attempt RT elevation on Linux when built with hardware; warn on failure
            #[cfg(all(target_os = "linux", feature = "hardware", feature = "rt"))]
            {
                use libc::{
                    SCHED_FIFO, sched_get_priority_max, sched_get_priority_min, sched_param,
                    sched_setscheduler,
                };
                unsafe {
                    let minp = sched_get_priority_min(SCHED_FIFO);
                    let maxp = sched_get_priority_max(SCHED_FIFO);
                    if minp < 0 || maxp < 0 || minp > maxp {
                        eprintln!("SCHED_FIFO not available; falling back to normal scheduling.");
                    } else {
                        // Request a low FIFO prio above min, clamped to [minp, maxp]
                        let mut req = minp.saturating_add(1);
                        if req > maxp {
                            req = maxp;
                        }
                        if req < minp {
                            req = minp;
                        }

                        let mut param = sched_param {
                            sched_priority: req,
                        };
                        let rc = sched_setscheduler(0, SCHED_FIFO, &mut param);
                        if rc != 0 {
                            let err = std::io::Error::last_os_error();
                            let code = err.raw_os_error().unwrap_or(0);
                            if code == libc::EPERM {
                                eprintln!(
                                    "Realtime scheduling denied (EPERM). Hint: needs CAP_SYS_NICE or root and an adequate RLIMIT_RTPRIO. ({err})"
                                );
                            } else if code == libc::EINVAL {
                                eprintln!(
                                    "Realtime scheduling failed (EINVAL). Hint: invalid parameters or unsupported policy/priority. ({err})"
                                );
                            } else {
                                eprintln!(
                                    "Realtime scheduling unavailable; expect higher jitter/overshoot. ({err})"
                                );
                            }
                        }
                    }
                }
            }

            // Repeatedly read scale to estimate HX711 SPS (10 vs 80) by inter-arrival time
            let timeout = Duration::from_millis(cfg.timeouts.sample_ms.max(1));
            let t_end = Instant::now() + Duration::from_millis(1000);
            let mut stamps = Vec::new();
            while Instant::now() < t_end {
                match scale.read(timeout) {
                    Ok(_v) => stamps.push(Instant::now()),
                    Err(e) => {
                        tracing::error!(error = %e, "scale read failed");
                        return Err(eyre::eyre!("scale read failed: {}", e));
                    }
                }
            }
            // Compute median delta
            let mut deltas_us: Vec<u64> = stamps
                .windows(2)
                .map(|w| (w[1] - w[0]).as_micros() as u64)
                .collect();
            deltas_us.sort_unstable();
            let median_us = if deltas_us.is_empty() {
                0
            } else {
                deltas_us[deltas_us.len() / 2]
            };
            // Classify: <50ms => 80 SPS, else 10 SPS (HX711 modes are ~12.5ms or ~100ms)
            let sps = if median_us < 50_000 { 80 } else { 10 };
            println!("Detected HX711 rate: {sps} SPS");
            Ok(())
        }
        Commands::Health => {
            tracing::info!("health check starting");
            use doser_traits::{Motor, Scale};
            use std::time::Duration;

            let (mut scale, mut motor) = hw;

            // Check scale responsiveness
            let scale_ok = match scale.read(Duration::from_millis(500)) {
                Ok(raw) => {
                    println!("✓ Scale: responsive (raw: {raw})");
                    true
                }
                Err(e) => {
                    eprintln!("✗ Scale: {e}");
                    false
                }
            };

            // Check motor (brief movement test)
            let motor_ok = match motor
                .set_speed(100)
                .and_then(|_| motor.start())
                .and_then(|_| {
                    std::thread::sleep(Duration::from_millis(50));
                    motor.stop()
                }) {
                Ok(_) => {
                    println!("✓ Motor: responsive");
                    true
                }
                Err(e) => {
                    eprintln!("✗ Motor: {e}");
                    false
                }
            };

            if scale_ok && motor_ok {
                println!("\nHealth check: OK");
                Ok(())
            } else {
                Err(eyre::eyre!("Health check failed"))
            }
        }
        Commands::Dose {
            grams,
            max_run_ms,
            max_overshoot_g,
            direct,
            print_runtime,
            rt,
            rt_prio,
            rt_lock,
            rt_cpu,
            stats,
        } => {
            // CLI flag overrides config; otherwise use config default
            let use_direct = if direct {
                true
            } else {
                match cfg.runner.mode {
                    doser_config::RunMode::Sampler => false,
                    doser_config::RunMode::Direct => true,
                }
            };
            let t0 = std::time::Instant::now();
            let res = run_dose(
                &cfg,
                calib.as_ref(),
                grams,
                max_run_ms,
                max_overshoot_g,
                use_direct,
                hw,
                rt,
                rt_prio,
                rt_lock,
                rt_cpu,
                stats,
                shutdown,
            );
            match res {
                Ok((final_g, tel)) => {
                    if print_runtime {
                        let ms = t0.elapsed().as_millis();
                        eprintln!("runtime: {ms} ms");
                    }
                    if cli.json {
                        use std::time::{SystemTime, UNIX_EPOCH};
                        let ts_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let profile =
                            std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
                        let slope = tel
                            .slope_ema_gps
                            .map(|v| format!("{v:.6}"))
                            .unwrap_or_else(|| "null".to_string());
                        let stop_at = tel
                            .stop_at_g
                            .map(|v| format!("{v:.3}"))
                            .unwrap_or_else(|| "null".to_string());
                        let coast = tel
                            .coast_comp_g
                            .map(|v| format!("{v:.3}"))
                            .unwrap_or_else(|| "null".to_string());
                        println!(
                            "{{\"timestamp\":{ts_ms},\"target_g\":{grams:.3},\"final_g\":{final_g:.3},\"duration_ms\":{},\"profile\":\"{}\",\"slope_ema\":{},\"stop_at_g\":{},\"coast_comp_g\":{},\"abort_reason\":null}}",
                            t0.elapsed().as_millis(),
                            profile,
                            slope,
                            stop_at,
                            coast
                        );
                    } else {
                        println!("final: {final_g:.2} g");
                    }
                    Ok(())
                }
                Err(e) => {
                    if cli.json {
                        use std::time::{SystemTime, UNIX_EPOCH};
                        let ts_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let profile =
                            std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
                        // Try to extract abort reason name
                        let abort = if let Some(doser_core::error::DoserError::Abort(reason)) =
                            e.downcast_ref::<doser_core::error::DoserError>()
                        {
                            abort_reason_name(reason)
                        } else {
                            "Error"
                        };
                        println!(
                            "{{\"timestamp\":{ts_ms},\"target_g\":{grams:.3},\"final_g\":null,\"duration_ms\":{},\"profile\":\"{}\",\"slope_ema\":null,\"stop_at_g\":null,\"coast_comp_g\":null,\"abort_reason\":\"{abort}\"}}",
                            t0.elapsed().as_millis(),
                            profile
                        );
                    }
                    Err(e)
                }
            }
        }
    }
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn run_dose(
    _cfg: &doser_config::Config,
    calib: Option<&Calibration>,
    grams: f32,
    max_run_ms_override: Option<u64>,
    max_overshoot_g_override: Option<f32>,
    direct: bool,
    hw: (
        impl doser_traits::Scale + Send + 'static,
        impl doser_traits::Motor + 'static,
    ),
    rt: bool,
    rt_prio: Option<i32>,
    rt_lock: Option<RtLock>,
    rt_cpu: Option<usize>,
    stats: bool,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> CoreResult<(f32, JsonTelemetry)> {
    // Real-time mode setup (Linux/macOS) — run once per process
    #[cfg(target_os = "linux")]
    {
        let mode = rt_lock.unwrap_or(RtLock::os_default());
        setup_rt_once(rt, rt_prio, mode, rt_cpu);
    }
    #[cfg(target_os = "macos")]
    {
        let mode = rt_lock.unwrap_or(RtLock::os_default());
        let _rt_prio = rt_prio; // silence unused on non-Linux builds
        let _rt_cpu = rt_cpu; // silence unused on non-Linux builds
        setup_rt_once(rt, mode);
    }

    // Stats: control loop latency, jitter, missed deadlines
    let mut latencies = Vec::new();
    let mut missed_deadlines = 0;
    let mut sample_count = 0;

    // Builder/config mapping
    let filter = doser_core::FilterCfg {
        ma_window: _cfg.filter.ma_window,
        median_window: _cfg.filter.median_window,
        sample_rate_hz: _cfg.filter.sample_rate_hz,
        ema_alpha: _cfg.filter.ema_alpha.unwrap_or(0.0),
    };
    let control = doser_core::ControlCfg {
        speed_bands: _cfg.control.speed_bands.clone(),
        coarse_speed: _cfg.control.coarse_speed,
        fine_speed: _cfg.control.fine_speed,
        slow_at_g: _cfg.control.slow_at_g,
        hysteresis_g: _cfg.control.hysteresis_g,
        stable_ms: _cfg.control.stable_ms,
        epsilon_g: _cfg.control.epsilon_g,
    };
    let timeouts = doser_core::Timeouts {
        sensor_ms: _cfg.timeouts.sample_ms,
    };
    let defaults = doser_core::SafetyCfg::default();
    let safety = doser_core::SafetyCfg {
        max_run_ms: max_run_ms_override.unwrap_or(if _cfg.safety.max_run_ms == 0 {
            defaults.max_run_ms
        } else {
            _cfg.safety.max_run_ms
        }),
        max_overshoot_g: max_overshoot_g_override.unwrap_or(
            if _cfg.safety.max_overshoot_g == 0.0 {
                defaults.max_overshoot_g
            } else {
                _cfg.safety.max_overshoot_g
            },
        ),
        no_progress_epsilon_g: _cfg.safety.no_progress_epsilon_g,
        no_progress_ms: _cfg.safety.no_progress_ms,
    };
    let _ = LAST_SAFETY.set(CliSafety {
        max_run_ms: safety.max_run_ms,
        max_overshoot_g: safety.max_overshoot_g,
        no_progress_ms: safety.no_progress_ms,
        no_progress_epsilon_g: safety.no_progress_epsilon_g,
    });
    let calibration_core = calib.map(|c| doser_core::Calibration {
        gain_g_per_count: c.scale_factor,
        zero_counts: c.offset,
        ..Default::default()
    });
    let (scale, motor) = hw;
    let estop_check: Option<Box<dyn Fn() -> bool + Send + Sync>> = {
        #[cfg(all(feature = "hardware", target_os = "linux"))]
        {
            if let Some(pin) = _cfg.pins.estop_in {
                match doser_hardware::make_estop_checker(
                    pin,
                    _cfg.estop.active_low,
                    _cfg.estop.poll_ms,
                ) {
                    Ok(c) => {
                        tracing::info!(
                            pin,
                            active_low = _cfg.estop.active_low,
                            poll_ms = _cfg.estop.poll_ms,
                            "E-stop enabled"
                        );
                        Some(c)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to init E-stop; continuing without it");
                        None
                    }
                }
            } else {
                None
            }
        }
        #[cfg(not(all(feature = "hardware", target_os = "linux")))]
        {
            let _ = &_cfg; // silence unused
            None
        }
    };
    let sampling_mode = if direct {
        SamplingMode::Direct
    } else {
        #[cfg(all(feature = "hardware", target_os = "linux"))]
        {
            SamplingMode::Event
        }
        #[cfg(not(all(feature = "hardware", target_os = "linux")))]
        {
            SamplingMode::Paced(_cfg.filter.sample_rate_hz)
        }
    };
    let prefer_timeout_first = max_run_ms_override.is_none();

    // Map predictor config
    let predictor_core = doser_core::PredictorCfg {
        enabled: _cfg.predictor.enabled,
        window: _cfg.predictor.window,
        extra_latency_ms: _cfg.predictor.extra_latency_ms,
        min_progress_ratio: _cfg.predictor.min_progress_ratio,
    };

    #[inline]
    fn record_sample(
        latencies: &mut Vec<u64>,
        missed_deadlines: &mut usize,
        period_us: u64,
        t_start: std::time::Instant,
    ) {
        let latency = t_start.elapsed().as_micros() as u64;
        latencies.push(latency);
        if latency > period_us {
            *missed_deadlines = missed_deadlines.saturating_add(1);
        }
    }

    // Stats collection for direct mode
    if matches!(sampling_mode, SamplingMode::Direct) && stats {
        // Direct mode: wrap control loop manually
        let estop_check_core: Option<Box<dyn Fn() -> bool>> =
            estop_check.map(|f| -> Box<dyn Fn() -> bool> { Box::new(f) });
        let mut doser = doser_core::build_doser(
            scale,
            motor,
            filter.clone(),
            control.clone(),
            safety.clone(),
            timeouts.clone(),
            calibration_core.clone(),
            grams,
            estop_check_core,
            Some(predictor_core.clone()),
            None,
            Some(_cfg.estop.debounce_n),
        )?;
        doser.begin();
        tracing::info!(target_g = grams, mode = "direct", "dose start");
        // Compute expected period only when collecting stats
        let period_us = doser_core::util::period_us(_cfg.filter.sample_rate_hz);
        loop {
            // Check for shutdown signal
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = doser.motor_stop();
                return Err(doser_core::error::DoserError::Abort(
                    doser_core::error::AbortReason::Estop,
                )
                .into());
            }

            let t_start = std::time::Instant::now();
            let status = doser.step()?;
            record_sample(&mut latencies, &mut missed_deadlines, period_us, t_start);
            sample_count += 1;
            match status {
                doser_core::DosingStatus::Running => continue,
                doser_core::DosingStatus::Complete => {
                    let final_g = doser.last_weight();
                    tracing::info!(final_g, "dose complete");
                    if stats && !latencies.is_empty() {
                        let expected_period_us =
                            doser_core::util::period_us(_cfg.filter.sample_rate_hz);
                        let min = *latencies.iter().min().unwrap_or(&0);
                        let max = *latencies.iter().max().unwrap_or(&0);
                        let avg = latencies.iter().sum::<u64>() as f64 / latencies.len() as f64;
                        let stdev = if latencies.len() > 1 {
                            let mean = avg;
                            let var = latencies
                                .iter()
                                .map(|&x| (x as f64 - mean).powi(2))
                                .sum::<f64>()
                                / (latencies.len() as f64 - 1.0);
                            var.sqrt()
                        } else {
                            0.0
                        };
                        eprintln!("\n--- Doser Stats ---");
                        eprintln!("Samples: {sample_count}");
                        eprintln!("Period (us): {expected_period_us}");
                        eprintln!(
                            "Latency min/avg/max/stdev (us): {min:.0} / {avg:.1} / {max:.0} / {stdev:.1}"
                        );
                        eprintln!("Missed deadlines (> period): {missed_deadlines}");
                        eprintln!("-------------------\n");
                    }
                    let tel = JsonTelemetry {
                        slope_ema_gps: doser.last_slope_ema_gps(),
                        stop_at_g: doser.early_stop_at_g(),
                        coast_comp_g: doser.last_inflight_g(),
                    };
                    return Ok((final_g, tel));
                }
                doser_core::DosingStatus::Aborted(e) => {
                    let _ = doser.motor_stop();
                    tracing::error!(error = %e, "dose aborted");
                    return Err(e.into());
                }
            }
        }
    } else if stats {
        // Sampler mode: wrap control loop manually
        let period_us = doser_core::util::period_us(_cfg.filter.sample_rate_hz);
        let sampler_timeout = std::time::Duration::from_millis(timeouts.sensor_ms);
        let sampler = match sampling_mode {
            SamplingMode::Event => doser_core::sampler::Sampler::spawn_event(
                scale,
                sampler_timeout,
                doser_traits::clock::MonotonicClock::new(),
            ),
            SamplingMode::Paced(hz) => doser_core::sampler::Sampler::spawn(
                scale,
                hz,
                sampler_timeout,
                doser_traits::clock::MonotonicClock::new(),
            ),
            SamplingMode::Direct => unreachable!(),
        };
        let estop_check_core: Option<Box<dyn Fn() -> bool>> =
            estop_check.map(|f| -> Box<dyn Fn() -> bool> { Box::new(f) });
        // Use shared NoopScale for sampler-driven mode
        use doser_core::mocks::NoopScale;
        let mut doser = doser_core::build_doser(
            NoopScale,
            motor,
            filter.clone(),
            control.clone(),
            safety.clone(),
            timeouts.clone(),
            calibration_core.clone(),
            grams,
            estop_check_core,
            Some(predictor_core.clone()),
            None,
            Some(_cfg.estop.debounce_n),
        )?;
        doser.begin();
        tracing::info!(target_g = grams, mode = "sampler", "dose start");
        loop {
            // Check for shutdown signal
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = doser.motor_stop();
                return Err(doser_core::error::DoserError::Abort(
                    doser_core::error::AbortReason::Estop,
                )
                .into());
            }

            let t_start = std::time::Instant::now();
            let status = if let Some(raw) = sampler.latest() {
                sample_count += 1;
                doser.step_from_raw(raw)?
            } else {
                std::thread::sleep(std::time::Duration::from_micros(period_us));
                continue;
            };
            record_sample(&mut latencies, &mut missed_deadlines, period_us, t_start);
            match status {
                doser_core::DosingStatus::Running => continue,
                doser_core::DosingStatus::Complete => {
                    let final_g = doser.last_weight();
                    tracing::info!(final_g, "dose complete");
                    if stats && !latencies.is_empty() {
                        let expected_period_us =
                            doser_core::util::period_us(_cfg.filter.sample_rate_hz);
                        let min = *latencies.iter().min().unwrap_or(&0);
                        let max = *latencies.iter().max().unwrap_or(&0);
                        let avg = latencies.iter().sum::<u64>() as f64 / latencies.len() as f64;
                        let stdev = if latencies.len() > 1 {
                            let mean = avg;
                            let var = latencies
                                .iter()
                                .map(|&x| (x as f64 - mean).powi(2))
                                .sum::<f64>()
                                / (latencies.len() as f64 - 1.0);
                            var.sqrt()
                        } else {
                            0.0
                        };
                        eprintln!("\n--- Doser Stats ---");
                        eprintln!("Samples: {sample_count}");
                        eprintln!("Period (us): {expected_period_us}");
                        eprintln!(
                            "Latency min/avg/max/stdev (us): {min:.0} / {avg:.1} / {max:.0} / {stdev:.1}"
                        );
                        eprintln!("Missed deadlines (> period): {missed_deadlines}");
                        eprintln!("-------------------\n");
                    }
                    let tel = JsonTelemetry {
                        slope_ema_gps: doser.last_slope_ema_gps(),
                        stop_at_g: doser.early_stop_at_g(),
                        coast_comp_g: doser.last_inflight_g(),
                    };
                    return Ok((final_g, tel));
                }
                doser_core::DosingStatus::Aborted(e) => {
                    let _ = doser.motor_stop();
                    tracing::error!(error = %e, "dose aborted");
                    return Err(e.into());
                }
            }
        }
    } else {
        // No stats: use core runner
        let final_g = doser_core::runner::run(
            scale,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration_core,
            grams,
            estop_check,
            _cfg.estop.debounce_n,
            prefer_timeout_first,
            sampling_mode,
            Some(predictor_core),
        )?;
        // Telemetry not available through runner; return nulls
        let tel = JsonTelemetry::default();
        return Ok((final_g, tel));
    }
    // Unreachable
    #[allow(unreachable_code)]
    Ok((0.0, JsonTelemetry::default()))
}

#[cfg(target_os = "linux")]
// Capacity of cpu_set_t in CPU indices (bits). cpu_set_t is a fixed-size bitset; its
// usable CPU index range is the number of bits it can hold. size_of returns bytes,
// so multiply by 8 to get bits.
const MAX_CPUSET_BITS: usize = std::mem::size_of::<libc::cpu_set_t>() * 8;

#[cfg(target_os = "linux")]
fn setup_rt_once(rt: bool, prio: Option<i32>, lock: RtLock, rt_cpu: Option<usize>) {
    use libc::{
        // CPU affinity helpers
        CPU_ISSET,
        CPU_SET,
        CPU_ZERO,
        SCHED_FIFO,
        // Memory locking (used by affinity helpers; mlockall imported inside helper if needed)
        // Scheduling (FIFO priority)
        sched_get_priority_max,
        sched_get_priority_min,
        sched_param,
        sched_setscheduler,
    };
    use std::sync::OnceLock;
    static RT_ONCE: OnceLock<()> = OnceLock::new();
    static ONLINE_CPUS: OnceLock<libc::c_long> = OnceLock::new();
    static CPUSET: OnceLock<libc::cpu_set_t> = OnceLock::new();

    if !rt {
        return;
    }

    // Apply process memory locking according to the selected mode.
    // Security/privileges: may require CAP_IPC_LOCK or a sufficient memlock ulimit.
    #[inline]
    fn try_apply_mem_lock(lock: RtLock) -> eyre::Result<()> {
        #[inline]
        fn is_retryable_memlock_error(err: &std::io::Error) -> bool {
            matches!(err.raw_os_error(), Some(code) if code == libc::EPERM || code == libc::ENOMEM)
        }
        use libc::{MCL_CURRENT, MCL_FUTURE, mlockall};

        // Helper: read current memlock rlimit for diagnostics
        #[inline]
        fn memlock_limit_hint() -> Option<String> {
            // Best-effort portability: use libc getrlimit when available
            // SAFETY: getrlimit writes into a stack-allocated rlimit; resource constant is valid.
            unsafe {
                let mut rlim = std::mem::MaybeUninit::<libc::rlimit>::uninit();
                let rc = libc::getrlimit(libc::RLIMIT_MEMLOCK, rlim.as_mut_ptr());
                if rc == 0 {
                    let r = rlim.assume_init();
                    let cur = r.rlim_cur;
                    if cur == libc::RLIM_INFINITY {
                        Some("memlock limit: unlimited".to_string())
                    } else {
                        Some(format!("memlock limit: {} KiB", cur / 1024))
                    }
                } else {
                    None
                }
            }
        }

        #[inline]
        fn lock_current() -> std::io::Result<()> {
            // SAFETY: mlockall is a pure syscall; no pointers passed.
            let rc = unsafe { mlockall(MCL_CURRENT) };
            if rc != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        #[inline]
        fn lock_all() -> std::io::Result<()> {
            // SAFETY: mlockall is a pure syscall; no pointers passed.
            let rc = unsafe { mlockall(MCL_CURRENT | MCL_FUTURE) };
            if rc != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        let attempted_all = matches!(lock, RtLock::All);
        let result: std::io::Result<()> = match lock {
            RtLock::None => Ok(()),
            RtLock::Current => lock_current(),
            RtLock::All => lock_all(),
        };
        if result.is_ok() {
            return Ok(());
        }
        let err = result
            .err()
            .unwrap_or_else(|| std::io::Error::other("mlockall failed"));

        // Fallback: if All failed due to permission or memory, try Current
        let mut fallback_err: Option<std::io::Error> = None;
        if attempted_all && is_retryable_memlock_error(&err) {
            match lock_current() {
                Ok(()) => return Ok(()),
                Err(e2) => fallback_err = Some(e2),
            }
        }

        // Build a more informative error message with hints
        let mut msg = format!(
            "mlockall({}) failed: {}",
            if attempted_all {
                "current|future"
            } else {
                "current"
            },
            err
        );
        if is_retryable_memlock_error(&err) {
            if let Some(h) = memlock_limit_hint() {
                msg.push_str(&format!("; {h}"));
            }
            msg.push_str("; hint: needs CAP_IPC_LOCK (or root) and sufficient 'ulimit -l'");
            if let Some(e2) = fallback_err {
                msg.push_str(&format!("; fallback mlockall(current) also failed: {e2}"));
            }
        }
        Err(eyre::eyre!(msg))
    }

    // Apply SCHED_FIFO priority, clamped to the system range.
    // Security/privileges: typically requires CAP_SYS_NICE or root.
    #[inline]
    fn try_apply_fifo_priority(prio: Option<i32>) -> eyre::Result<()> {
        // Check for CAP_SYS_NICE capability (best effort)
        #[cfg(target_os = "linux")]
        {
            use std::fs;
            // Read /proc/self/status to check for CAP_SYS_NICE
            if let Ok(status) = fs::read_to_string("/proc/self/status") {
                let has_cap = status.lines().any(|line| {
                    if line.starts_with("CapEff:") || line.starts_with("CapPrm:") {
                        // CAP_SYS_NICE is bit 23 (0x800000)
                        if let Some(hex) = line.split_whitespace().nth(1)
                            && let Ok(caps) = u64::from_str_radix(hex, 16)
                        {
                            return caps & 0x800000 != 0;
                        }
                    }
                    false
                });

                if !has_cap {
                    // Check if we're root as fallback
                    let is_root = unsafe { libc::geteuid() == 0 };
                    if !is_root {
                        return Err(eyre::eyre!(
                            "Insufficient privileges for SCHED_FIFO: needs CAP_SYS_NICE or root. \
                            Current effective UID: {}. \
                            Hint: Run with 'sudo' or grant CAP_SYS_NICE: 'sudo setcap cap_sys_nice=ep /path/to/doser'",
                            unsafe { libc::geteuid() }
                        ));
                    }
                }
            }
        }

        // SAFETY: calls query priority range without pointers.
        let (min, max) = unsafe {
            let min = sched_get_priority_min(SCHED_FIFO);
            let max = sched_get_priority_max(SCHED_FIFO);
            if min < 0 || max < 0 {
                (1, 99)
            } else {
                (min, max)
            }
        };
        let wanted = prio.unwrap_or(max);
        let prio_val = wanted.clamp(min, max);
        let param = sched_param {
            sched_priority: prio_val,
        };
        // SAFETY: sched_setscheduler takes a pointer to our stack `param`; lifetime is valid for the call.
        // SAFETY: sched_setscheduler takes a pointer to our stack param; PID 0 = current process.
        let rc = unsafe { sched_setscheduler(0, SCHED_FIFO, &param) };
        if rc != 0 {
            Err(eyre::eyre!(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    // Pin process to a single CPU if permitted by the current affinity mask.
    // Respects cgroup/container restrictions via sched_getaffinity.
    #[inline]
    fn try_apply_affinity(
        rt_cpu: Option<usize>,
        online_cpus: &OnceLock<libc::c_long>,
        mask: &OnceLock<libc::cpu_set_t>,
    ) -> eyre::Result<()> {
        // Capacity reference: see module-level MAX_CPUSET_BITS.
        // Cache online CPUs
        let _ = online_cpus.get_or_init(|| {
            // SAFETY: sysconf(_SC_NPROCESSORS_ONLN) returns a scalar; no side-effects.
            unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) }
        });
        // Get current allowed mask; on failure, fallback to [0..online_cpus)
        let _ = mask.get_or_init(|| {
            // SAFETY: zeroed cpu_set_t is a valid initial mask structure
            let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
            // SAFETY: CPU_ZERO initializes the mask memory we provide
            unsafe { CPU_ZERO(&mut set) };
            // SAFETY: sched_getaffinity writes up to size_of::<cpu_set_t>() into &mut set
            let rc = unsafe {
                libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set)
            };
            if rc != 0 {
                // Reset to empty, then mark CPUs [0..n) as allowed (clamped to cpuset capacity)
                // SAFETY: pointer valid, structure on stack
                unsafe { CPU_ZERO(&mut set) };
                let n = online_cpus
                    .get()
                    .copied()
                    .unwrap_or_else(|| unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) });
                let n = if n < 0 { 0 } else { n as usize };
                let n = n.min(MAX_CPUSET_BITS);
                for i in 0..n {
                    // SAFETY: i < capacity ensures CPU_SET stays within the bitset
                    unsafe { CPU_SET(i, &mut set) };
                }
            }
            set
        });
        let nprocs_onln = *online_cpus.get().unwrap_or(&0);
        if nprocs_onln < 1 {
            eyre::bail!("_SC_NPROCESSORS_ONLN < 1");
        }
        let target = rt_cpu.unwrap_or(0);
        if target as libc::c_long >= nprocs_onln {
            eyre::bail!("requested CPU {target} >= online {nprocs_onln}");
        }
        if target >= MAX_CPUSET_BITS {
            eyre::bail!("requested CPU {target} exceeds cpu_set_t capacity {MAX_CPUSET_BITS}");
        }
        let Some(allowed) = mask.get() else {
            eyre::bail!("cpuset init failed");
        };
        // SAFETY: target < max_cpuset_bits and `allowed` points to a valid cpu_set_t.
        // Normalize CPU_ISSET return type across libc variants (bool vs c_int).
        let allowed_target = unsafe { (CPU_ISSET(target, allowed) as libc::c_int) != 0 };
        if !allowed_target {
            eyre::bail!("CPU {target} not permitted by current affinity mask");
        }
        // SAFETY: zeroed cpu_set_t becomes a valid mask; CPU_SET index is checked above.
        let mut desired: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe {
            CPU_ZERO(&mut desired);
            CPU_SET(target, &mut desired);
        }
        // SAFETY: sched_setaffinity reads size_of::<cpu_set_t>() bytes from &desired
        let rc =
            unsafe { libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &desired) };
        if rc != 0 {
            Err(eyre::eyre!(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    RT_ONCE.get_or_init(|| {
        // Memory lock
        match try_apply_mem_lock(lock) {
            Ok(()) => match lock {
                RtLock::None => eprintln!("RT: memory locking disabled (none)"),
                RtLock::Current => eprintln!("RT: memory lock = current"),
                RtLock::All => eprintln!("RT: memory lock = all (current|future)"),
            },
            Err(err) => eprintln!("Warning: mlockall failed: {err}"),
        }
        // FIFO priority
        if let Err(err) = try_apply_fifo_priority(prio) {
            let prio_dbg = prio
                .map(|p| p.to_string())
                .unwrap_or_else(|| "(max)".into());
            eprintln!("Warning: sched_setscheduler(SCHED_FIFO, prio={prio_dbg}) failed: {err}");
        }
        // Affinity
        if let Err(err) = try_apply_affinity(rt_cpu, &ONLINE_CPUS, &CPUSET) {
            eprintln!("Warning: affinity not applied: {err}");
        }
    });
}

#[cfg(target_os = "macos")]
fn setup_rt_once(rt: bool, lock: RtLock) {
    use libc::{MCL_CURRENT, MCL_FUTURE, mlockall};
    use std::sync::OnceLock;
    static RT_ONCE: OnceLock<()> = OnceLock::new();
    if !rt {
        return;
    }
    RT_ONCE.get_or_init(|| {
        match lock {
            RtLock::None => {
                eprintln!("RT: memory locking disabled (none)");
            }
            RtLock::Current => {
                let rc = unsafe { mlockall(MCL_CURRENT) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    eprintln!("Warning: mlockall(MCL_CURRENT) failed: {err}");
                } else {
                    eprintln!("RT: memory lock = current");
                }
            }
            RtLock::All => {
                let rc = unsafe { mlockall(MCL_CURRENT | MCL_FUTURE) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    eprintln!("Warning: mlockall(MCL_CURRENT|MCL_FUTURE) failed: {err}");
                } else {
                    eprintln!("RT: memory lock = all (current|future)");
                }
            }
        }
        eprintln!("Warning: macOS does not support SCHED_FIFO or affinity; only mlockall applied.");
    });
}

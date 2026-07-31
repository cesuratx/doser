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
//! - Offer `--json` mode emitting stable JSONL result lines to stdout; all log
//!   records (json or pretty) go to stderr, so stdout carries only CLI output
//! - Provide optional RT helpers via libc on supported OSes, with safety docs
//! - Map domain abort reasons to stable exit codes

mod cli;
mod dose;
mod error_fmt;
mod jog;
mod monitor;
mod rt;
mod tracing_setup;

use std::fs;

use clap::Parser;
use doser_config::{Calibration, Config, load_calibration_csv};
use eyre::WrapErr;
use serde_json::json;

use cli::{Cli, Commands, JSON_MODE};
use dose::abort_reason_name;
use error_fmt::{exit_code_for_error, format_error_json, humanize};
use tracing_setup::init_tracing;

fn main() {
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
            println!("{}", format_error_json(&e));
        } else {
            eprintln!("{}", humanize(&e));
        }
        std::process::exit(code);
    }
}

/// How long the `motor` jog should run for the requested `--steps`/`--ms`.
///
/// `--steps N` is duration-derived sugar: the motor is paced by its stepping
/// thread, so the only lever we have is wall-clock time at the *effective* rate.
/// Two things matter here:
/// - the rate is clamped to [`doser_hardware::MAX_STEP_RATE_SPS`] first, because
///   both backends clamp `set_speed`; deriving the duration from an unclamped
///   rate would silently execute a fraction of the requested steps;
/// - the division rounds **up**, so the count is never short by the truncated
///   remainder (100 steps at 300 sps is 334 ms, not 333).
///
/// `--sps` is range-checked by clap to `1..=MAX_STEP_RATE_SPS`, so the zero-rate
/// division can't happen; the clamp below is belt-and-braces.
fn jog_duration(steps: Option<u32>, sps: u32, ms: u64) -> std::time::Duration {
    let effective_sps = sps.clamp(1, doser_hardware::MAX_STEP_RATE_SPS);
    steps.map_or_else(
        || std::time::Duration::from_millis(ms),
        |n| {
            std::time::Duration::from_millis(
                (u64::from(n) * 1000).div_ceil(u64::from(effective_sps)),
            )
        },
    )
}

/// Refuse to even read a config file larger than this: real configs are a few KB,
/// and this keeps a hostile or corrupt file from being slurped into memory.
const MAX_CONFIG_BYTES: u64 = 1 << 20; // 1 MiB

/// True when the monitor's `--bind` value keeps the server on this machine.
/// Anything we can't parse as a loopback IP is treated as externally reachable,
/// so the exposure warning errs on the side of being printed.
fn is_loopback_bind(bind: &str) -> bool {
    bind.parse::<std::net::IpAddr>().map_or_else(
        |_| bind.eq_ignore_ascii_case("localhost"),
        |ip| ip.is_loopback(),
    )
}

/// Read, size-check, parse and validate the TOML config named on the command line.
fn load_config(cli: &Cli) -> eyre::Result<Config> {
    if let Ok(meta) = fs::metadata(&cli.config)
        && meta.len() > MAX_CONFIG_BYTES
    {
        eyre::bail!(
            "config file {} is too large ({} bytes > {} byte limit)",
            cli.config.display(),
            meta.len(),
            MAX_CONFIG_BYTES
        );
    }
    let cfg_text = fs::read_to_string(&cli.config)
        .wrap_err_with(|| format!("read config {}", cli.config.display()))?;
    let cfg: Config = toml::from_str(&cfg_text)
        .wrap_err_with(|| format!("parse config {}", cli.config.display()))?;

    // Validate configuration with clear errors
    cfg.validate().wrap_err("invalid configuration")?;
    Ok(cfg)
}

/// Persisted calibration in the config wins over `--calibration` CSV; neither is required.
fn load_calibration(cli: &Cli, cfg: &mut Config) -> eyre::Result<Option<Calibration>> {
    if let Some(pc) = cfg.calibration.take() {
        // Use the From impl so the persisted additive `offset_g` is preserved
        // (manual field construction previously dropped it).
        return Ok(Some(Calibration::from(pc)));
    }
    cli.calibration.as_ref().map_or_else(
        || Ok(None),
        |p| {
            load_calibration_csv(p)
                .map(Some)
                .map_err(|e| eyre::eyre!("parse calibration {:?}: {}", p, e))
        },
    )
}

fn real_main(shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>) -> eyre::Result<()> {
    let cli = Cli::parse();
    let _ = JSON_MODE.set(cli.json);

    // 1) Load typed config from TOML (with a size cap so a huge file can't OOM)
    let mut cfg = load_config(&cli)?;

    init_tracing(
        cli.json,
        &cli.log_level,
        cfg.logging.file.as_deref(),
        cfg.logging.rotation.as_deref(),
    );

    // 2) Load calibration: prefer persisted in TOML if present; else optional CSV
    let calib = load_calibration(&cli, &mut cfg)?;

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
    // Linked sim pair so the simulated scale responds to the simulated motor.
    let hw = doser_hardware::sim_pair();

    match cli.cmd {
        Commands::SelfCheck => {
            let (scale, _motor) = hw;
            cmd_self_check(scale, &cfg)
        }
        Commands::Health => {
            let (scale, motor) = hw;
            cmd_health(scale, motor)
        }
        Commands::Monitor { port, bind, hz } => {
            let (scale, _motor) = hw;
            cmd_monitor(scale, calib, &cfg, &bind, port, hz, &shutdown)
        }
        Commands::Motor {
            sps,
            ms,
            steps,
            dir,
        } => {
            let (_scale, motor) = hw;
            jog::run(
                motor,
                sps,
                dir.is_clockwise(),
                jog_duration(steps, sps, ms),
                &shutdown,
            )
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
        } => cmd_dose(
            &DoseArgs {
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
            },
            &cfg,
            calib.as_ref(),
            hw,
            cli.json,
            shutdown,
        ),
    }
}

/// Flags for the `dose` subcommand, grouped so the handler keeps a sane signature.
//
// `struct_excessive_bools` allowed: this mirrors the clap subcommand's flags one for
// one. Folding them into enums here would only move the boolean-ness up into `Commands`.
#[allow(clippy::struct_excessive_bools)]
struct DoseArgs {
    grams: f32,
    max_run_ms: Option<u64>,
    max_overshoot_g: Option<f32>,
    direct: bool,
    print_runtime: bool,
    rt: bool,
    rt_prio: Option<i32>,
    rt_lock: Option<cli::RtLock>,
    rt_cpu: Option<usize>,
    stats: bool,
}

/// `self-check`: read the scale for a second and classify the HX711 sample rate.
fn cmd_self_check<S: doser_traits::Scale>(scale: S, cfg: &Config) -> eyre::Result<()> {
    use std::time::{Duration, Instant};

    tracing::info!("self-check starting");
    let mut scale = scale;

    // Attempt RT elevation on Linux when built with hardware
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
                let mut req = minp.saturating_add(1);
                if req > maxp {
                    req = maxp;
                }
                if req < minp {
                    req = minp;
                }

                // `sched_setscheduler` takes a `*const sched_param`; a shared borrow
                // is enough.
                let param = sched_param {
                    sched_priority: req,
                };
                let rc = sched_setscheduler(0, SCHED_FIFO, &param);
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
    let t_end = Instant::now() + Duration::from_secs(1);
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
        .map(|w| u64::try_from((w[1] - w[0]).as_micros()).unwrap_or(u64::MAX))
        .collect();
    deltas_us.sort_unstable();
    let median_us = if deltas_us.is_empty() {
        0
    } else {
        deltas_us[deltas_us.len() / 2]
    };
    // Classify: <50ms => 80 SPS, else 10 SPS
    let sps = if median_us < 50_000 { 80 } else { 10 };
    println!("Detected HX711 rate: {sps} SPS");
    Ok(())
}

/// `health`: prove the scale answers and the motor spins, without dosing anything.
fn cmd_health<S: doser_traits::Scale, M: doser_traits::Motor>(
    mut scale: S,
    mut motor: M,
) -> eyre::Result<()> {
    use std::time::Duration;

    tracing::info!("health check starting");

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

    let motor_ok = match motor
        .set_speed(100)
        .and_then(|()| motor.start())
        .and_then(|()| {
            std::thread::sleep(Duration::from_millis(50));
            motor.stop()
        }) {
        Ok(()) => {
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

/// `monitor`: serve the live weight UI, warning first if it is not bound to loopback.
fn cmd_monitor<S: doser_traits::Scale + Send + 'static>(
    scale: S,
    calib: Option<Calibration>,
    cfg: &Config,
    bind: &str,
    port: u16,
    hz: Option<u32>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> eyre::Result<()> {
    // Binding anything but loopback publishes an unauthenticated, unencrypted
    // live view of the scale to every host that can route to this machine.
    // That is the intended default on a bench LAN, but say so out loud.
    if !is_loopback_bind(bind) {
        tracing::warn!(bind = %bind, port, "monitor bound to a non-loopback address");
        eprintln!(
            "WARNING: monitor is binding {bind}:{port} — the UI has no authentication \
             and no TLS, so anyone who can reach this machine can watch the scale. \
             Use --bind 127.0.0.1 to keep it local."
        );
    }
    let sample_hz = hz.unwrap_or(cfg.filter.sample_rate_hz);
    // Generous per-read timeout: at 10 SPS the data-ready gap is ~90 ms.
    let read_timeout =
        std::time::Duration::from_millis(cfg.hardware.sensor_read_timeout_ms.max(200));
    monitor::run(scale, calib, sample_hz, read_timeout, bind, port, shutdown)
}

/// `dose`: run one dose and emit either the JSONL result line or the plain summary.
fn cmd_dose<S, M>(
    args: &DoseArgs,
    cfg: &Config,
    calib: Option<&Calibration>,
    hw: (S, M),
    json_mode: bool,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> eyre::Result<()>
where
    S: doser_traits::Scale + Send + 'static,
    M: doser_traits::Motor + 'static,
{
    let &DoseArgs {
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
    } = args;

    let use_direct = if direct {
        true
    } else {
        match cfg.runner.mode {
            doser_config::RunMode::Sampler => false,
            doser_config::RunMode::Direct => true,
        }
    };
    let t0 = std::time::Instant::now();
    let res = dose::run_dose(
        cfg,
        calib,
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
            if json_mode {
                use std::time::{SystemTime, UNIX_EPOCH};
                let ts_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
                let obj = json!({
                    "timestamp": ts_ms,
                    "target_g": format!("{grams:.3}").parse::<f64>().unwrap_or(0.0),
                    "final_g": format!("{final_g:.3}").parse::<f64>().unwrap_or(0.0),
                    "duration_ms": u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "profile": profile,
                    "slope_ema": tel.slope_ema_gps,
                    "stop_at_g": tel.stop_at_g,
                    "coast_comp_g": tel.coast_comp_g,
                    "abort_reason": serde_json::Value::Null
                });
                println!("{obj}");
            } else {
                println!("final: {final_g:.2} g");
            }
            Ok(())
        }
        Err(e) => {
            if json_mode {
                use std::time::{SystemTime, UNIX_EPOCH};
                let ts_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
                let abort = if let Some(doser_core::error::DoserError::Abort(reason)) =
                    e.downcast_ref::<doser_core::error::DoserError>()
                {
                    abort_reason_name(reason)
                } else {
                    "Error"
                };
                let obj = json!({
                    "timestamp": ts_ms,
                    "target_g": format!("{grams:.3}").parse::<f64>().unwrap_or(0.0),
                    "final_g": serde_json::Value::Null,
                    "duration_ms": u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "profile": profile,
                    "slope_ema": serde_json::Value::Null,
                    "stop_at_g": serde_json::Value::Null,
                    "coast_comp_g": serde_json::Value::Null,
                    "abort_reason": abort
                });
                println!("{obj}");
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_loopback_bind, jog_duration};
    use std::time::Duration;

    #[test]
    fn jog_duration_falls_back_to_ms_without_steps() {
        assert_eq!(jog_duration(None, 200, 1500), Duration::from_millis(1500));
    }

    #[test]
    fn jog_duration_rounds_up_so_steps_are_not_short() {
        // 100 steps at 300 sps is 333.33 ms; truncation used to shave a step.
        assert_eq!(
            jog_duration(Some(100), 300, 1000),
            Duration::from_millis(334)
        );
        assert_eq!(
            jog_duration(Some(200), 200, 1000),
            Duration::from_millis(1000)
        );
    }

    #[test]
    fn jog_duration_uses_the_clamped_rate() {
        // The motor caps at MAX_STEP_RATE_SPS, so the duration must too: at the
        // unclamped 20_000 sps this would have been 5 ms and stepped ~25 times.
        let capped = jog_duration(Some(100), doser_hardware::MAX_STEP_RATE_SPS, 1000);
        assert_eq!(jog_duration(Some(100), 20_000, 1000), capped);
        assert_eq!(capped, Duration::from_millis(20));
    }

    #[test]
    fn loopback_binds_are_recognized() {
        assert!(is_loopback_bind("127.0.0.1"));
        assert!(is_loopback_bind("::1"));
        assert!(is_loopback_bind("localhost"));
        assert!(!is_loopback_bind("0.0.0.0"));
        assert!(!is_loopback_bind("192.168.1.42"));
        assert!(!is_loopback_bind("not-an-address"));
    }
}

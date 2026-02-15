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

mod cli;
mod dose;
mod error_fmt;
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

    // 1) Load typed config from TOML
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

            let (mut scale, _motor) = hw;

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
            // Classify: <50ms => 80 SPS, else 10 SPS
            let sps = if median_us < 50_000 { 80 } else { 10 };
            println!("Detected HX711 rate: {sps} SPS");
            Ok(())
        }
        Commands::Health => {
            tracing::info!("health check starting");
            use doser_traits::{Motor, Scale};
            use std::time::Duration;

            let (mut scale, mut motor) = hw;

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
                        let obj = json!({
                            "timestamp": ts_ms,
                            "target_g": format!("{grams:.3}").parse::<f64>().unwrap_or(0.0),
                            "final_g": format!("{final_g:.3}").parse::<f64>().unwrap_or(0.0),
                            "duration_ms": t0.elapsed().as_millis() as u64,
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
                    if cli.json {
                        use std::time::{SystemTime, UNIX_EPOCH};
                        let ts_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let profile =
                            std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
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
                            "duration_ms": t0.elapsed().as_millis() as u64,
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
    }
}

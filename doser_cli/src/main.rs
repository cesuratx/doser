use std::{fs, path::PathBuf};

use clap::{ArgAction, Parser, Subcommand};
use doser_config::{load_calibration_csv, Calibration, Config};
use doser_core::error::Result as CoreResult;
use eyre::WrapErr;

use std::sync::OnceLock;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use doser_core::runner::SamplingMode;

// Local NoopScale for sampler-driven mode (removed; orchestration moved to core runner)

static FILE_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

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
        if let DoserError::State(s) = de {
            let lower = s.to_ascii_lowercase();
            if lower.contains("estop") {
                return "What happened: Emergency stop was triggered.\nLikely causes: E-stop button pressed or input pin active.\nHow to fix: Release E-stop, ensure wiring is correct, then start a new run.".to_string();
            }
            if lower.contains("no progress") {
                return "What happened: No progress watchdog tripped.\nLikely causes: Jammed auger, empty hopper, or scale not changing within threshold.\nHow to fix: Check mechanics and materials; adjust safety.no_progress_* in config if needed.".to_string();
            }
            if lower.contains("max run time exceeded") {
                return "max run time was exceeded.\nLikely causes: Too conservative speeds, high target, or stalls.\nHow to fix: Increase safety.max_run_ms or adjust speeds/target.".to_string();
            }
            if lower.contains("max overshoot exceeded") {
                return "What happened: Overshoot beyond safety limit.\nLikely causes: Inertia or too high coarse/fine speed near target.\nHow to fix: Lower speeds or increase safety.max_overshoot_g and tune epsilon/slow_at.".to_string();
            }
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

    /// Log level: trace,debug,info,warn,error
    #[arg(long, value_name = "LEVEL", default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a dose to a target in grams
    Dose {
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
        #[arg(long, action = ArgAction::SetTrue)]
        rt: bool,
        /// Print control loop and sampling stats
        #[arg(long, action = ArgAction::SetTrue)]
        stats: bool,
    },
    /// Quick health check (hardware presence / sim ok)
    SelfCheck,
}

fn main() -> eyre::Result<()> {
    if let Err(e) = real_main() {
        eprintln!("{}", humanize(&e));
        std::process::exit(2);
    }
    Ok(())
}

fn real_main() -> eyre::Result<()> {
    let cli = Cli::parse();

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

    // 2) Load calibration if provided
    let calib: Option<Calibration> = match &cli.calibration {
        Some(p) => {
            let c = load_calibration_csv(p)
                .map_err(|e| eyre::eyre!("parse calibration {:?}: {}", p, e))?;
            Some(c)
        }
        None => None,
    };

    // 3) Build hardware (feature-gated) or sim
    #[cfg(feature = "hardware")]
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

    #[cfg(not(feature = "hardware"))]
    let hw = {
        use doser_hardware::{SimulatedMotor, SimulatedScale};
        (SimulatedScale::new(), SimulatedMotor::default())
    };

    match cli.cmd {
        Commands::SelfCheck => {
            tracing::info!("self-check starting");
            use doser_traits::{Motor, Scale};
            use std::time::Duration;

            // Move hw so we can probe it here; the process exits after SelfCheck.
            let (mut scale, mut motor) = hw;

            // Probe scale read with configured timeout
            match scale.read(Duration::from_millis(cfg.timeouts.sample_ms)) {
                Ok(_v) => tracing::info!("scale read ok"),
                Err(e) => {
                    tracing::error!(error = %e, "scale read failed");
                    return Err(eyre::eyre!("scale read failed: {}", e));
                }
            }

            // Probe motor lifecycle (non-destructive): start -> set_speed(0) -> stop
            if let Err(e) = motor.start() {
                tracing::error!(error = %e, "motor start failed");
                return Err(eyre::eyre!("motor start failed: {}", e));
            }
            let _ = motor.set_speed(0); // ignore errors here, stop will follow
            if let Err(e) = motor.stop() {
                tracing::error!(error = %e, "motor stop failed");
                return Err(eyre::eyre!("motor stop failed: {}", e));
            }

            tracing::info!("self-check ok");
            println!("OK");
            Ok(())
        }
        Commands::Dose {
            grams,
            max_run_ms,
            max_overshoot_g,
            direct,
            print_runtime,
            rt,
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
                stats,
            );
            res?;
            if print_runtime {
                let ms = t0.elapsed().as_millis();
                eprintln!("runtime: {ms} ms");
            }
            Ok(())
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
    stats: bool,
) -> CoreResult<()> {
    // Real-time mode setup (Linux/macOS)
    #[cfg(target_os = "linux")]
    if rt {
        use libc::{
            mlockall, sched_param, sched_setscheduler, CPU_SET, CPU_ZERO, MCL_CURRENT, MCL_FUTURE,
            SCHED_FIFO,
        };
        unsafe {
            mlockall(MCL_CURRENT | MCL_FUTURE);
        }
        let mut param = sched_param { sched_priority: 99 };
        unsafe {
            sched_setscheduler(0, SCHED_FIFO, &mut param);
        }
        // Optionally set affinity to CPU 0
        // libc::CPU_ZERO/CPU_SET expect &mut cpu_set_t on Linux
        let mut cpuset: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe {
            CPU_ZERO(&mut cpuset);
            CPU_SET(0, &mut cpuset);
            libc::sched_setaffinity(
                0,
                std::mem::size_of::<libc::cpu_set_t>(),
                &cpuset,
            );
        }
    }
    #[cfg(target_os = "macos")]
    if rt {
        use libc::mlockall;
        unsafe {
            mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        }
        eprintln!("Warning: macOS does not support SCHED_FIFO or affinity; only mlockall applied.");
    }

    // Stats: control loop latency, jitter, missed deadlines
    let mut latencies = Vec::new();
    let mut missed_deadlines = 0;
    let mut sample_count = 0;
    let expected_period_us = (1_000_000u64 / _cfg.filter.sample_rate_hz.max(1) as u64).max(1);

    // Builder/config mapping
    let filter = doser_core::FilterCfg {
        ma_window: _cfg.filter.ma_window,
        median_window: _cfg.filter.median_window,
        sample_rate_hz: _cfg.filter.sample_rate_hz,
    };
    let control = doser_core::ControlCfg {
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
    let calibration_core = calib.map(|c| doser_core::Calibration {
        gain_g_per_count: c.scale_factor,
        zero_counts: c.offset,
        ..Default::default()
    });
    let (scale, motor) = hw;
    let estop_check: Option<Box<dyn Fn() -> bool + Send + Sync>> = {
        #[cfg(feature = "hardware")]
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
        #[cfg(not(feature = "hardware"))]
        {
            let _ = &_cfg; // silence unused
            None
        }
    };
    let sampling_mode = if direct {
        SamplingMode::Direct
    } else {
        #[cfg(feature = "hardware")]
        {
            SamplingMode::Event
        }
        #[cfg(not(feature = "hardware"))]
        {
            SamplingMode::Paced(_cfg.filter.sample_rate_hz)
        }
    };
    let prefer_timeout_first = max_run_ms_override.is_none();

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
            None,
            Some(_cfg.estop.debounce_n),
        )?;
        doser.begin();
        tracing::info!(target_g = grams, mode = "direct", "dose start");
        loop {
            let t_start = std::time::Instant::now();
            let status = doser.step()?;
            let t_end = std::time::Instant::now();
            let latency = t_end.duration_since(t_start).as_micros() as u64;
            latencies.push(latency);
            if latency > expected_period_us {
                missed_deadlines += 1;
            }
            sample_count += 1;
            match status {
                doser_core::DosingStatus::Running => continue,
                doser_core::DosingStatus::Complete => {
                    let final_g = doser.last_weight();
                    tracing::info!(final_g, "dose complete");
                    println!("final: {final_g:.2} g");
                    break;
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
        let period_us = expected_period_us;
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
        // Local NoopScale for sampler-driven mode
        struct NoopScale;
        impl doser_traits::Scale for NoopScale {
            fn read(
                &mut self,
                _timeout: std::time::Duration,
            ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
                Err(Box::new(std::io::Error::other("noop scale")))
            }
        }
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
            None,
            Some(_cfg.estop.debounce_n),
        )?;
        doser.begin();
        tracing::info!(target_g = grams, mode = "sampler", "dose start");
        loop {
            let t_start = std::time::Instant::now();
            let status = if let Some(raw) = sampler.latest() {
                sample_count += 1;
                doser.step_from_raw(raw)?
            } else {
                std::thread::sleep(std::time::Duration::from_micros(period_us));
                continue;
            };
            let t_end = std::time::Instant::now();
            let latency = t_end.duration_since(t_start).as_micros() as u64;
            latencies.push(latency);
            if latency > period_us {
                missed_deadlines += 1;
            }
            match status {
                doser_core::DosingStatus::Running => continue,
                doser_core::DosingStatus::Complete => {
                    let final_g = doser.last_weight();
                    tracing::info!(final_g, "dose complete");
                    println!("final: {final_g:.2} g");
                    break;
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
        )?;
        println!("final: {final_g:.2} g");
    }

    if stats && !latencies.is_empty() {
        let min = *latencies.iter().min().unwrap();
        let max = *latencies.iter().max().unwrap();
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
        eprintln!("Latency min/avg/max/stdev (us): {min:.0} / {avg:.1} / {max:.0} / {stdev:.1}");
        eprintln!("Missed deadlines (> period): {missed_deadlines}");
        eprintln!("-------------------\n");
    }
    Ok(())
}

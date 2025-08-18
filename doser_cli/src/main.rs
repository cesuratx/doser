use std::{fs, path::PathBuf};

use anyhow::Context;
use clap::{ArgAction, Parser, Subcommand};
use doser_config::{load_calibration_csv, Calibration, Config};
use doser_core::{error::Result as CoreResult, Doser, DosingStatus};

use std::sync::OnceLock;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static FILE_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

/// Initialize tracing once for the whole app.
fn init_tracing(json: bool, level: &str, file: Option<&str>, rotation: Option<&str>) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    let base = tracing_subscriber::registry().with(filter);

    if json {
        let console = fmt::layer().json().with_target(false);
        if let Some(path) = file {
            // Ensure directory exists and allow switching to rotation if needed.
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
            let file_layer = fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(nb_writer);
            base.with(console).with(file_layer).init();
        } else {
            base.with(console).init();
        }
    } else {
        let console = fmt::layer().pretty().with_target(false);
        if let Some(path) = file {
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
            let file_layer = fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(nb_writer);
            base.with(console).with(file_layer).init();
        } else {
            base.with(console).init();
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
    },
    /// Quick health check (hardware presence / sim ok)
    SelfCheck,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 1) Load typed config from TOML (for logging.file)
    let cfg_text =
        fs::read_to_string(&cli.config).with_context(|| format!("read config {:?}", cli.config))?;
    let cfg: Config =
        toml::from_str(&cfg_text).with_context(|| format!("parse config {:?}", cli.config))?;

    init_tracing(
        cli.json,
        &cli.log_level,
        cfg.logging.file.as_deref(),
        cfg.logging.rotation.as_deref(),
    );

    // 2) Load calibration if provided
    let calib: Option<Calibration> = match &cli.calibration {
        Some(p) => {
            let c =
                load_calibration_csv(p).with_context(|| format!("parse calibration {:?}", p))?;
            Some(c)
        }
        None => None,
    };

    // 3) Build hardware (feature-gated) or sim
    #[cfg(feature = "hardware")]
    let hw = {
        use doser_hardware::{HardwareMotor, HardwareScale};
        let scale =
            HardwareScale::try_new(cfg.pins.hx711_dt, cfg.pins.hx711_sck).context("open HX711")?;
        let motor = HardwareMotor::try_new_with_en(
            cfg.pins.motor_step,
            cfg.pins.motor_dir,
            cfg.pins.motor_en,
        )
        .context("open motor")?;
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
                    println!("SCALE_ERROR: {e}");
                    return Ok(());
                }
            }

            // Probe motor lifecycle (non-destructive): start -> set_speed(0) -> stop
            if let Err(e) = motor.start() {
                tracing::error!(error = %e, "motor start failed");
                println!("MOTOR_ERROR: {e}");
                return Ok(());
            }
            let _ = motor.set_speed(0); // ignore errors here, stop will follow
            if let Err(e) = motor.stop() {
                tracing::error!(error = %e, "motor stop failed");
                println!("MOTOR_ERROR: {e}");
                return Ok(());
            }

            tracing::info!("self-check ok");
            println!("OK");
            Ok(())
        }
        Commands::Dose {
            grams,
            max_run_ms,
            max_overshoot_g,
        } => run_dose(&cfg, calib.as_ref(), grams, max_run_ms, max_overshoot_g, hw)
            .with_context(|| "dose failed"),
    }
}

#[allow(clippy::type_complexity)]
fn run_dose(
    _cfg: &doser_config::Config,
    calib: Option<&Calibration>,
    grams: f32,
    // CLI safety overrides
    max_run_ms_override: Option<u64>,
    max_overshoot_g_override: Option<f32>,
    // 'static bounds so these can be boxed inside the Doser builder:
    hw: (
        impl doser_traits::Scale + 'static,
        impl doser_traits::Motor + 'static,
    ),
) -> CoreResult<()> {
    // Map typed config into core config objects
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
    };
    let timeouts = doser_core::Timeouts {
        sensor_ms: _cfg.timeouts.sample_ms,
    };
    let defaults = doser_core::SafetyCfg::default();
    let safety = doser_core::SafetyCfg {
        max_run_ms: max_run_ms_override.unwrap_or_else(|| {
            if _cfg.safety.max_run_ms == 0 {
                defaults.max_run_ms
            } else {
                _cfg.safety.max_run_ms
            }
        }),
        max_overshoot_g: max_overshoot_g_override.unwrap_or_else(|| {
            if _cfg.safety.max_overshoot_g == 0.0 {
                defaults.max_overshoot_g
            } else {
                _cfg.safety.max_overshoot_g
            }
        }),
        no_progress_epsilon_g: _cfg.safety.no_progress_epsilon_g,
        no_progress_ms: _cfg.safety.no_progress_ms,
    };

    let mut builder = Doser::builder()
        .with_scale(hw.0)
        .with_motor(hw.1)
        .with_filter(filter)
        .with_control(control)
        .with_safety(safety)
        .with_timeouts(timeouts)
        .with_target_grams(grams);

    // Hardware E-stop hookup (feature-gated). Create a real GPIO-backed checker.
    #[cfg(feature = "hardware")]
    {
        if let Some(estop_pin) = _cfg.pins.estop_in {
            // Active-low E-stop is common; poll every 5ms.
            match doser_hardware::make_estop_checker(estop_pin, true, 5) {
                Ok(checker) => {
                    // move the boxed closure into the builder; wrap to erase Send+Sync bound
                    builder = builder.with_estop_check(move || checker());
                }
                Err(e) => {
                    // If we cannot create the checker, proceed without E-stop but log it.
                    tracing::warn!("failed to init estop checker: {}", e);
                }
            }
        }
    }

    // Apply calibration CSV if provided
    if let Some(c) = calib {
        builder = builder
            .with_tare_counts(c.offset)
            .with_calibration_gain_offset(c.scale_factor, 0.0);
    }

    let mut doser = builder.build()?;

    tracing::info!(target_g = grams, "dose start");

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match doser.step()? {
            DosingStatus::Running => continue,
            DosingStatus::Complete => {
                let final_g = doser.last_weight();
                tracing::info!(final_g, attempts, "dose complete");
                println!("final: {final_g:.2} g  attempts: {attempts}");
                return Ok(());
            }
            DosingStatus::Aborted(e) => {
                let _ = doser.motor_stop();
                tracing::error!(error = %e, attempts, "dose aborted");
                return Err(e);
            }
        }
    }
}

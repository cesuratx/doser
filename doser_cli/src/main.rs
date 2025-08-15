use std::{fs, path::PathBuf};

use anyhow::Context;
use clap::{ArgAction, Parser, Subcommand};
use doser_config::{load_calibration_csv, Calibration, Config};
use doser_core::{error::Result as CoreResult, Doser, DosingStatus};
use doser_traits; // for trait names in function signatures

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize tracing once for the whole app.
fn init_tracing(json: bool, level: &str) {
    // EnvFilter supports strings like "info", "debug", or module-level filters.
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    if json {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().with_target(false))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().pretty().with_target(false))
            .init();
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
    },
    /// Quick health check (hardware presence / sim ok)
    SelfCheck,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.json, &cli.log_level);

    // 1) Load typed config from TOML
    let cfg_text =
        fs::read_to_string(&cli.config).with_context(|| format!("read config {:?}", cli.config))?;
    let cfg: Config =
        toml::from_str(&cfg_text).with_context(|| format!("parse config {:?}", cli.config))?;

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
    let mut hw = {
        use doser_hardware::{HardwareMotor, HardwareScale};
        let scale =
            HardwareScale::try_new(cfg.pins.hx711_dt, cfg.pins.hx711_sck).context("open HX711")?;
        let motor = HardwareMotor::try_new(cfg.pins.motor_step, cfg.pins.motor_dir)
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
            // Minimal probe: on hardware builds try a short read;
            // on sim builds just succeed.
            #[cfg(feature = "hardware")]
            {
                use doser_traits::Scale;
                use std::time::Duration;
                let mut hw_mut = hw; // need mutable to call read()
                let _ = hw_mut
                    .0
                    .read(Duration::from_millis(cfg.timeouts.sensor_ms))
                    .context("scale read")?;
            }
            tracing::info!("self-check ok");
            println!("OK");
            Ok(())
        }
        Commands::Dose { grams } => {
            run_dose(&cfg, calib.as_ref(), grams, hw).with_context(|| "dose failed")
        }
    }
}

#[allow(clippy::type_complexity)]
fn run_dose(
    _cfg: &doser_config::Config,
    calib: Option<&Calibration>,
    grams: f32,
    // 'static bounds so these can be boxed inside the Doser builder:
    hw: (
        impl doser_traits::Scale + 'static,
        impl doser_traits::Motor + 'static,
    ),
) -> CoreResult<()> {
    // For now, pass core defaults for filter/control/timeouts.
    // We can map _cfg fields into core types later once names are aligned.
    let mut doser = Doser::builder()
        .with_scale(hw.0)
        .with_motor(hw.1)
        .with_filter(doser_core::FilterCfg::default())
        .with_control(doser_core::ControlCfg::default())
        .with_timeouts(doser_core::Timeouts::default())
        .with_target_grams(grams)
        .apply_calibration(calib) // generic; core stays config-agnostic
        .build()?;

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

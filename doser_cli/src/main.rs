use clap::Parser;
use csv::ReaderBuilder;
use doser_core::config::{ConfigSource, CsvConfigSource, TomlConfigSource};
use doser_core::{DoserError, dose_to_target, log_dosing_result, render_progress_bar};
use std::collections::HashMap;
// ...existing code...
use eyre::{Result, WrapErr};
use std::fs;
use std::path::Path;

/// Simple CLI for bean doser
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Target grams to dose
    #[arg(short, long)]
    grams: Option<f32>,

    /// Calibrate with known weight (in grams)
    #[arg(long)]
    calibrate: Option<f32>,

    /// HX711 DT pin
    #[arg(long)]
    dt_pin: Option<u8>,

    /// HX711 SCK pin
    #[arg(long)]
    sck_pin: Option<u8>,

    /// Stepper STEP pin
    #[arg(long)]
    step_pin: Option<u8>,

    /// Stepper DIR pin
    #[arg(long)]
    dir_pin: Option<u8>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Choose hardware or simulation
    #[cfg(feature = "hardware")]
    let mut scale: Box<dyn doser_hardware::Scale> = Box::new(doser_hardware::HardwareScale::new(
        args.dt_pin.unwrap_or(5),
        args.sck_pin.unwrap_or(6),
    ));
    #[cfg(not(feature = "hardware"))]
    let mut scale: Box<dyn doser_hardware::Scale> = Box::new(doser_hardware::SimulatedScale::new());

    #[cfg(feature = "hardware")]
    let mut motor: Box<dyn doser_hardware::Motor> = Box::new(doser_hardware::HardwareMotor::new(
        args.step_pin.unwrap_or(13),
        args.dir_pin.unwrap_or(19),
    ));
    #[cfg(not(feature = "hardware"))]
    let mut motor: Box<dyn doser_hardware::Motor> = Box::new(doser_hardware::SimulatedMotor);

    if let Some(known_weight) = args.calibrate {
        if known_weight <= 0.0 {
            eyre::bail!("Calibration weight must be positive.");
        }
        scale.tare();
        println!("Place known weight (e.g., calibration mass) on scale...");
        std::thread::sleep(std::time::Duration::from_secs(2));
        scale.calibrate(known_weight);
        println!("Calibration finished.");
        return Ok(());
    }

    // Load config sources (TOML and CSV)
    let mut config_sources: Vec<Box<dyn ConfigSource>> = Vec::new();
    let config_path = Path::new("doser_config.toml");
    if config_path.exists() {
        let content =
            fs::read_to_string(config_path).wrap_err_with(|| "Failed to read config file")?;
        if let Ok(val) = toml::from_str(&content) {
            config_sources.push(Box::new(TomlConfigSource(val)));
        }
    }
    let csv_path = Path::new("doser_config.csv");
    if csv_path.exists() {
        let mut csv_config: HashMap<String, String> = HashMap::new();
        let mut rdr = ReaderBuilder::new()
            .has_headers(false)
            .from_path(csv_path)?;
        for result in rdr.records() {
            let record = result?;
            if record.len() >= 2 {
                let key = record[0].trim().to_string();
                let value = record[1].trim().to_string();
                if !key.starts_with('#') && !key.is_empty() {
                    csv_config.insert(key, value);
                }
            }
        }
        config_sources.push(Box::new(CsvConfigSource(csv_config)));
    }

    // Helper to get config value from sources (CSV > TOML)
    fn get_config_u8(sources: &[Box<dyn ConfigSource>], key: &str) -> Option<u8> {
        for src in sources {
            if let Some(v) = src.get_u8(key) {
                return Some(v);
            }
        }
        None
    }
    fn get_config_u32(sources: &[Box<dyn ConfigSource>], key: &str) -> Option<u32> {
        for src in sources {
            if let Some(v) = src.get_u32(key) {
                return Some(v);
            }
        }
        None
    }

    // Use builder pattern for dosing session
    let mut builder = doser_core::DosingSessionBuilder::new();
    if let Some(g) = args.grams {
        builder = builder.target_grams(g);
    }
    builder = builder
        .max_attempts(
            get_config_u32(&config_sources, "max_attempts")
                .map(|v| v as usize)
                .unwrap_or(100),
        )
        .dt_pin(
            args.dt_pin
                .or_else(|| get_config_u8(&config_sources, "hx711_dt_pin"))
                .unwrap_or(5),
        )
        .sck_pin(
            args.sck_pin
                .or_else(|| get_config_u8(&config_sources, "hx711_sck_pin"))
                .unwrap_or(6),
        )
        .step_pin(
            args.step_pin
                .or_else(|| get_config_u8(&config_sources, "stepper_step_pin"))
                .unwrap_or(13),
        )
        .dir_pin(
            args.dir_pin
                .or_else(|| get_config_u8(&config_sources, "stepper_dir_pin"))
                .unwrap_or(19),
        );

    let session = match builder.build() {
        Some(s) => s,
        None => {
            println!("Please specify --grams <value> to dose and all required pins.");
            return Ok(());
        }
    };

    println!(
        "Using pins: HX711 DT={} SCK={}, Stepper STEP={} DIR={}",
        session.dt_pin, session.sck_pin, session.step_pin, session.dir_pin
    );

    if args.grams.is_some() {
        println!("Target grams: {:.2}", session.target_grams);
        scale.tare();
        motor.start();
        let result = dose_to_target(
            session.target_grams,
            || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                scale.read_weight()
            },
            session.max_attempts,
        );
        motor.stop();
        match result {
            Ok(dosing) => {
                let bar = render_progress_bar(dosing.final_weight, session.target_grams, 20);
                log_dosing_result(
                    "dosing_log.txt",
                    &dosing,
                    session.target_grams,
                    session.dt_pin,
                    session.sck_pin,
                    session.step_pin,
                    session.dir_pin,
                    None,
                );
                println!("\rDosing: {}", bar);
                println!("Dosing complete in {} attempts.", dosing.attempts);
                // Demonstrate iterator ADT usage
                let mut step_iter = session.steps(|| {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    scale.read_weight()
                });
                println!("Step-by-step weights:");
                while let Some((attempt, weight)) = step_iter.next() {
                    println!("Attempt {}: {:.2}g", attempt, weight);
                }
            }
            Err(e) => match e {
                DoserError::NegativeTarget => eyre::bail!("Target grams must be positive."),
                DoserError::MaxAttemptsExceeded => {
                    eyre::bail!("Dosing failed, weight did not reach target in reasonable time.")
                }
                DoserError::NegativeWeight => eyre::bail!("Negative weight reading detected."),
            },
        }
    } else {
        println!(
            "Please specify --grams <value> to dose or --calibrate <known_weight> to calibrate."
        );
    }
    Ok(())
}

use clap::Parser;
use csv::ReaderBuilder;
use doser_core::config::{ConfigSource, CsvConfigSource, TomlConfigSource};
use doser_core::logger::Logger;
use doser_core::render_progress_bar;
use std::collections::HashMap;
// ...existing code...
use eyre::{Result, WrapErr};
use std::fs;
use std::path::Path;

/// Simple CLI for bean doser
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Log file path (optional)
    #[arg(long)]
    log: Option<String>,
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
    let mut calibration_source: Option<Box<dyn doser_core::calibration::CalibrationSource>> = None;
    let mut avg_scale_factor: Option<f32> = None;
    if csv_path.exists() {
        let mut csv_config: HashMap<String, String> = HashMap::new();
        let mut calibration_data: Vec<(f32, f32)> = Vec::new();
        let mut rdr = ReaderBuilder::new()
            .has_headers(false)
            .from_path(csv_path)?;
        for result in rdr.records() {
            let record = result?;
            if record.len() >= 2 {
                let key = record[0].trim().to_string();
                let value = record[1].trim().to_string();
                if !key.starts_with('#') && !key.is_empty() {
                    // Calibration data: numeric key and value
                    if let (Ok(w), Ok(r)) = (key.parse::<f32>(), value.parse::<f32>()) {
                        calibration_data.push((w, r));
                    } else {
                        csv_config.insert(key, value);
                    }
                }
            }
        }
        config_sources.push(Box::new(CsvConfigSource(csv_config)));
        if !calibration_data.is_empty() {
            calibration_source = Some(Box::new(
                doser_core::calibration::CsvCalibrationSource::new(calibration_data.clone()),
            ));
            // Calculate average scale factor from calibration data
            let factors: Vec<f32> = calibration_data
                .iter()
                .map(|(w, r)| doser_core::calculate_scale_factor(*w, *r))
                .collect();
            if !factors.is_empty() {
                avg_scale_factor =
                    Some(factors.iter().copied().sum::<f32>() / factors.len() as f32);
            }
        }
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

    let log_path = args
        .log
        .clone()
        .unwrap_or_else(|| "dosing_log.txt".to_string());
    let logger = doser_core::logger::FileLogger::new(log_path);
    if let Some(cal_src) = &calibration_source {
        let cal_data = cal_src.get_calibration();
        logger.log(&format!("Calibration data loaded: {:?}", cal_data));
        if let Some(avg) = avg_scale_factor {
            logger.log(&format!("Average scale factor: {:.4}", avg));
        }
    }

    if args.grams.is_some() {
        println!("Target grams: {:.2}", session.target_grams);
        scale.tare();
        motor.start();
        // Create Doser instance with moving average filter (window size 5)
        let mut doser = doser_core::Doser::new(
            scale,
            motor,
            session.target_grams,
            5, // moving average window size
        );
        let mut attempts = 0;
        let mut weights: Vec<f32> = Vec::new();
        loop {
            // Step-wise dosing control
            match doser.step() {
                Ok(status) => {
                    attempts += 1;
                    let filtered_weight = doser.filtered_weight();
                    weights.push(filtered_weight);
                    let bar = render_progress_bar(filtered_weight, session.target_grams, 20);
                    print!("\rDosing: {} | {:.2}g", bar, filtered_weight);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    match status {
                        doser_core::DosingStatus::Complete => {
                            println!("");
                            break;
                        }
                        doser_core::DosingStatus::Running => {}
                        doser_core::DosingStatus::Error => {
                            println!("");
                            eyre::bail!("Dosing failed: error status returned");
                        }
                    }
                }
                Err(e) => {
                    println!("");
                    eyre::bail!("Dosing failed: {}", e);
                }
            }
        }
        let log_msg = format!(
            "target: {:.2}, final: {:.2}, attempts: {}, pins: DT={}, SCK={}, STEP={}, DIR={}, calibration: {:?}",
            session.target_grams,
            doser.filtered_weight(),
            attempts,
            session.dt_pin,
            session.sck_pin,
            session.step_pin,
            session.dir_pin,
            Option::<f32>::None
        );
        logger.log(&log_msg);
        println!("Dosing complete in {} attempts.", attempts);
        println!("Step-by-step weights:");
        for (i, w) in weights.iter().enumerate() {
            println!("Attempt {}: {:.2}g", i + 1, w);
        }
    } else {
        println!(
            "Please specify --grams <value> to dose or --calibrate <known_weight> to calibrate."
        );
    }
    Ok(())
}

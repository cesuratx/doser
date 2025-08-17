use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Pins {
    pub hx711_dt: u8,
    pub hx711_sck: u8,
    pub motor_step: u8,
    pub motor_dir: u8,
    pub estop_in: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct FilterCfg {
    pub ma_window: usize,
    pub median_window: usize,
    pub sample_rate_hz: u32,
}

#[derive(Debug, Deserialize)]
pub struct ControlCfg {
    pub coarse_speed: u32,
    pub fine_speed: u32,
    pub slow_at_g: f32,
    pub hysteresis_g: f32,
    pub stable_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct Timeouts {
    pub sample_ms: u64,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Safety {
    pub max_run_ms: u64,
    pub max_overshoot_g: f32,
    // Abort if weight change < epsilon for at least this many ms (0 disables)
    pub no_progress_epsilon_g: f32,
    pub no_progress_ms: u64,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Logging {
    pub file: Option<String>,  // path to .log (JSON lines)
    pub level: Option<String>, // "info","debug"
    /// Log rotation policy: "never" | "daily" | "hourly" (default: never)
    pub rotation: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub pins: Pins,
    pub filter: FilterCfg,
    pub control: ControlCfg,
    pub timeouts: Timeouts,
    #[serde(default)]
    pub safety: Safety,
    #[serde(default)]
    pub logging: Logging,
}

pub fn load_toml(s: &str) -> Result<Config, toml::de::Error> {
    toml::from_str::<Config>(s)
}

#[derive(Debug)]
pub struct Calibration {
    pub offset: i32,
    pub scale_factor: f32,
}

pub fn load_calibration_csv(path: &std::path::Path) -> std::io::Result<Calibration> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path)?;
    let mut offset = None;
    let mut scale_factor = None;

    for rec in rdr.deserialize::<(String, String, String)>() {
        let (kind, key, value) = rec?;
        if kind == "scale" && key == "offset" {
            offset = Some(
                value
                    .parse::<i32>()
                    .map_err(|_| std::io::ErrorKind::InvalidData)?,
            );
        } else if kind == "scale" && key == "scale_factor" {
            scale_factor = Some(
                value
                    .parse::<f32>()
                    .map_err(|_| std::io::ErrorKind::InvalidData)?,
            );
        }
    }

    Ok(Calibration {
        offset: offset.ok_or(std::io::ErrorKind::InvalidData)?,
        scale_factor: scale_factor.ok_or(std::io::ErrorKind::InvalidData)?,
    })
}

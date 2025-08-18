use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Pins {
    pub hx711_dt: u8,
    pub hx711_sck: u8,
    pub motor_step: u8,
    pub motor_dir: u8,
    pub motor_en: Option<u8>,
    pub estop_in: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct FilterCfg {
    pub ma_window: usize,
    pub median_window: usize,
    pub sample_rate_hz: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ControlCfg {
    pub coarse_speed: u32,
    pub fine_speed: u32,
    pub slow_at_g: f32,
    pub hysteresis_g: f32,
    pub stable_ms: u64,
    /// Additional control epsilon in grams used for stability/approach decisions
    pub epsilon_g: f32,
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
#[serde(default)]
pub struct Hardware {
    /// Max time to wait for HX711 data-ready (DT low) before failing
    pub sensor_read_timeout_ms: u64,
}

impl Default for Hardware {
    fn default() -> Self {
        Self {
            sensor_read_timeout_ms: 150,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub pins: Pins,
    pub filter: FilterCfg,
    #[serde(default)]
    pub control: ControlCfg,
    pub timeouts: Timeouts,
    #[serde(default)]
    pub safety: Safety,
    #[serde(default)]
    pub logging: Logging,
    #[serde(default)]
    pub hardware: Hardware,
}

pub fn load_toml(s: &str) -> Result<Config, toml::de::Error> {
    toml::from_str::<Config>(s)
}

impl Default for ControlCfg {
    fn default() -> Self {
        Self {
            coarse_speed: 1200,
            fine_speed: 250,
            slow_at_g: 1.0,
            hysteresis_g: 0.05,
            stable_ms: 250,
            epsilon_g: 0.0,
        }
    }
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

impl Config {
    pub fn validate(&self) -> eyre::Result<()> {
        // Control
        if self.control.coarse_speed == 0 {
            eyre::bail!("control.coarse_speed must be > 0");
        }
        if self.control.fine_speed == 0 {
            eyre::bail!("control.fine_speed must be > 0");
        }
        if self.control.slow_at_g.is_sign_negative() {
            eyre::bail!("control.slow_at_g must be >= 0");
        }
        if self.control.hysteresis_g.is_sign_negative() {
            eyre::bail!("control.hysteresis_g must be >= 0");
        }
        if self.control.stable_ms > 5 * 60 * 1000 {
            eyre::bail!("control.stable_ms is unreasonably large (>5min)");
        }
        if self.control.epsilon_g < 0.0 || self.control.epsilon_g > 1.0 {
            eyre::bail!("control.epsilon_g must be in [0.0, 1.0]");
        }

        // Safety
        if self.safety.max_overshoot_g < 0.0 {
            eyre::bail!("safety.max_overshoot_g must be >= 0.0");
        }
        if self.safety.no_progress_epsilon_g < 0.0 || self.safety.no_progress_epsilon_g > 1.0 {
            eyre::bail!("safety.no_progress_epsilon_g must be in [0.0, 1.0]");
        }
        if self.safety.no_progress_ms > 24 * 60 * 60 * 1000 {
            eyre::bail!("safety.no_progress_ms is unreasonably large (>24h)");
        }

        // Filter
        if self.filter.ma_window == 0 {
            eyre::bail!("filter.ma_window must be >= 1");
        }
        if self.filter.median_window == 0 {
            eyre::bail!("filter.median_window must be >= 1");
        }
        if self.filter.sample_rate_hz == 0 {
            eyre::bail!("filter.sample_rate_hz must be > 0");
        }

        // Timeouts
        if self.timeouts.sample_ms == 0 {
            eyre::bail!("timeouts.sample_ms must be >= 1");
        }

        // Hardware
        if self.hardware.sensor_read_timeout_ms == 0 {
            eyre::bail!("hardware.sensor_read_timeout_ms must be >= 1");
        }

        Ok(())
    }
}

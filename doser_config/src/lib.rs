use serde::Deserialize;

/// Calibration CSV schema.
///
/// Expected headers:
/// raw,grams
///
/// Example:
/// raw,grams
/// 842913,0.0
/// 1024913,100.0
#[derive(Debug, Deserialize, Clone, Copy)]
pub struct CalibrationRow {
    pub raw: i64,
    pub grams: f32,
}

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

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Safety {
    pub max_run_ms: u64,
    pub max_overshoot_g: f32,
    // Abort if weight change < epsilon for at least this many ms (0 disables)
    pub no_progress_epsilon_g: f32,
    pub no_progress_ms: u64,
}

impl Default for Safety {
    fn default() -> Self {
        Self {
            max_run_ms: 0,
            max_overshoot_g: 0.0,
            no_progress_epsilon_g: 0.02,
            no_progress_ms: 1200,
        }
    }
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

impl Calibration {
    /// Build Calibration from at least two rows: grams ~= gain * (raw - offset)
    pub fn from_rows(rows: Vec<CalibrationRow>) -> eyre::Result<Self> {
        if rows.len() < 2 {
            eyre::bail!("calibration requires at least two rows, got {}", rows.len());
        }
        let r1 = rows[0];
        let r2 = rows[1];
        if (r2.raw - r1.raw) == 0 {
            eyre::bail!("calibration rows have identical raw values; cannot compute scale factor");
        }
        let dr = (r2.raw - r1.raw) as f32;
        let dg = r2.grams - r1.grams;
        let gain = dg / dr; // grams per count
        if !gain.is_finite() || gain == 0.0 {
            eyre::bail!("calibration produced invalid gain: {}", gain);
        }
        let offset_f = r1.raw as f32 - (r1.grams / gain);
        let offset_i32 = offset_f.round() as i32;
        Ok(Calibration {
            offset: offset_i32,
            scale_factor: gain,
        })
    }
}

pub fn load_calibration_csv(path: &std::path::Path) -> eyre::Result<Calibration> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path)
        .map_err(|e| eyre::eyre!("open calibration CSV {:?}: {}", path, e))?;

    // Enforce exact headers
    let headers = rdr
        .headers()
        .map_err(|e| eyre::eyre!("read CSV headers {:?}: {}", path, e))?
        .clone();
    let expected = ["raw", "grams"];
    let actual: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
    if actual != expected {
        eyre::bail!(
            "calibration CSV must have headers 'raw,grams', got: {}",
            actual.join(",")
        );
    }

    let mut rows = Vec::new();
    for (idx, rec) in rdr.deserialize::<CalibrationRow>().enumerate() {
        match rec {
            Ok(row) => rows.push(row),
            Err(e) => {
                eyre::bail!("invalid CSV row {}: {}", idx + 2, e);
            }
        }
    }

    Calibration::from_rows(rows)
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
        if self.safety.no_progress_epsilon_g <= 0.0
            || self.safety.no_progress_epsilon_g > 1.0
        {
            eyre::bail!("safety.no_progress_epsilon_g must be in (0.0, 1.0]");
        }
        if self.safety.no_progress_ms == 0 {
            eyre::bail!("safety.no_progress_ms must be >= 1");
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

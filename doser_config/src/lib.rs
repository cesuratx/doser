#![cfg_attr(all(not(debug_assertions), not(test)), deny(warnings))]
#![cfg_attr(
    all(not(debug_assertions), not(test)),
    deny(clippy::all, clippy::pedantic, clippy::nursery)
)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]
//! Config schemas and calibration parsing for the dosing system.
//!
//! - `Config` and sub-structs are deserialized from TOML and validated.
//! - Calibration CSV loader enforces headers and performs a robust refit
//!   to reduce outlier influence before slope/intercept estimation.
use serde::Deserialize;
use serde::de::Deserializer;

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
    /// Optional EMA smoothing factor; when set, EMA is used in the core smoothing stage.
    /// Range: (0.0, 1.0]. If absent or <= 0, EMA is disabled.
    pub ema_alpha: Option<f32>,
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
    /// Optional speed table. Accepts either:
    /// - array of tables: [{ threshold_g = 1.0, sps = 1100 }, ...]
    /// - array of tuples: [[1.0, 1100], [0.5, 450], ...]
    #[serde(default, deserialize_with = "de_speed_bands")]
    pub speed_bands: Vec<(f32, u32)>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Timeouts {
    /// Sampling timeout per read (ms). Also accepts alias "sensor_ms".
    #[serde(alias = "sensor_ms")]
    pub sample_ms: u64,
    /// Optional settle window override mistakenly placed under [timeouts] in some configs.
    /// Parsed and ignored to keep backward compatibility.
    #[serde(default)]
    pub settle_ms: Option<u64>,
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
#[serde(default)]
pub struct EstopCfg {
    /// Treat low level as pressed when true
    pub active_low: bool,
    /// Number of consecutive polls required to latch E-stop
    pub debounce_n: u8,
    /// Polling interval in milliseconds for GPIO E-stop checker
    pub poll_ms: u64,
}

impl Default for EstopCfg {
    fn default() -> Self {
        Self {
            active_low: true,
            debounce_n: 2,
            poll_ms: 5,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    #[default]
    Sampler,
    Direct,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RunnerCfg {
    /// Default orchestration mode: "sampler" (event/rate-paced) or "direct"
    pub mode: RunMode,
}

impl Default for RunnerCfg {
    fn default() -> Self {
        Self {
            mode: RunMode::Sampler,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PredictorCfg {
    /// Enable early-stop predictor to reduce overshoot under latency
    pub enabled: bool,
    /// Rolling window size (samples) for slope estimate
    pub window: usize,
    /// Extra latency margin to account for filtering/IO (ms)
    pub extra_latency_ms: u64,
    /// Minimum fraction of target progress before predictor activates (0.0..=1.0)
    pub min_progress_ratio: f32,
}

impl Default for PredictorCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            window: 6,
            extra_latency_ms: 20,
            min_progress_ratio: 0.10,
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
    /// Early-stop predictor configuration
    #[serde(default)]
    pub predictor: PredictorCfg,
    /// Emergency stop configuration
    #[serde(default)]
    pub estop: EstopCfg,
    /// Runner/orchestration defaults
    #[serde(default)]
    pub runner: RunnerCfg,
    /// Optional persisted calibration; preferred at runtime over CSV when present.
    #[serde(default)]
    pub calibration: Option<PersistedCalibration>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct PersistedCalibration {
    /// grams per count
    pub gain_g_per_count: f32,
    /// tare zero in raw counts
    pub zero_counts: i32,
    /// additive offset in grams (rarely needed; default 0.0)
    #[serde(default)]
    pub offset_g: f32,
}

impl From<PersistedCalibration> for Calibration {
    fn from(p: PersistedCalibration) -> Self {
        Calibration {
            offset: p.zero_counts,
            scale_factor: p.gain_g_per_count,
        }
    }
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
            speed_bands: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BandToml {
    Tuple((f32, u32)),
    Table { threshold_g: f32, sps: u32 },
}

fn de_speed_bands<'de, D>(deserializer: D) -> Result<Vec<(f32, u32)>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<BandToml>> = Option::deserialize(deserializer)?;
    let mut out = Vec::new();
    if let Some(items) = opt {
        for b in items {
            match b {
                BandToml::Tuple((thr, sps)) => out.push((thr, sps)),
                BandToml::Table { threshold_g, sps } => out.push((threshold_g, sps)),
            }
        }
    }
    Ok(out)
}

#[derive(Debug)]
pub struct Calibration {
    pub offset: i32,
    pub scale_factor: f32,
}

impl Calibration {
    /// Build Calibration from calibration rows using ordinary least squares on all points.
    /// Fits grams = a*raw + b, then converts to core form grams = a*(raw - offset) + 0,
    /// where offset = round(-b/a) is the tare baseline in raw counts.
    pub fn from_rows(rows: Vec<CalibrationRow>) -> eyre::Result<Self> {
        if rows.len() < 2 {
            eyre::bail!("calibration requires at least two rows, got {}", rows.len());
        }

        // Ensure strictly monotonic raw values (increasing or decreasing), no duplicates
        let mut dir: i8 = 0; // 1 for increasing, -1 for decreasing
        for i in 1..rows.len() {
            let d = rows[i].raw - rows[i - 1].raw;
            if d == 0 {
                eyre::bail!(
                    "calibration rows have duplicate raw values at index {} and {}",
                    i - 1,
                    i
                );
            }
            let step_dir = if d > 0 { 1 } else { -1 };
            if dir == 0 {
                dir = step_dir;
            } else if dir != step_dir {
                eyre::bail!(
                    "calibration raw values must be monotonic (strictly increasing or strictly decreasing)"
                );
            }
        }

        // Closure: OLS fit in f64 for numerical stability
        let fit = |pts: &[(i64, f32)]| -> eyre::Result<(f64, f64)> {
            let n = pts.len() as f64;
            let sum_x: f64 = pts.iter().map(|r| r.0 as f64).sum();
            let sum_y: f64 = pts.iter().map(|r| r.1 as f64).sum();
            let mean_x = sum_x / n;
            let mean_y = sum_y / n;
            let mut sxx = 0.0f64;
            let mut sxy = 0.0f64;
            for (rx, gy) in pts {
                let x = *rx as f64 - mean_x;
                let y = *gy as f64 - mean_y;
                sxx += x * x;
                sxy += x * y;
            }
            if !sxx.is_finite() || sxx == 0.0 {
                eyre::bail!("calibration cannot determine slope (degenerate X variance)");
            }
            let a = sxy / sxx;
            if !a.is_finite() {
                eyre::bail!("calibration produced non-finite slope");
            }
            if a == 0.0 {
                eyre::bail!("calibration produced zero slope (invalid scale factor)");
            }
            let b = mean_y - a * mean_x;
            Ok((a, b))
        };

        // Initial fit
        let pts: Vec<(i64, f32)> = rows.iter().map(|r| (r.raw, r.grams)).collect();
        let (a0, b0) = fit(&pts)?;
        // Compute robust sigma estimate (RMS of residuals) without allocating residuals
        let mut sumsq: f64 = 0.0;
        for (x, y) in &pts {
            let r = (*y as f64) - (a0 * (*x as f64) + b0);
            sumsq += r * r;
        }
        let n_pts = pts.len();
        let rms = if n_pts == 0 {
            0.0
        } else {
            (sumsq / (n_pts as f64)).sqrt()
        };

        // Reject outliers with |residual| > 2σ and refit if at least 2 remain.
        let (a, b) = robust_refit(&pts, a0, b0, rms, 2.0).unwrap_or((a0, b0));

        // Convert to core representation: grams = a * (raw - offset) + 0
        let zero_counts = if a == 0.0 {
            eyre::bail!("calibration slope is zero, cannot compute tare baseline");
        } else {
            -b / a // where grams==0
        };
        if !zero_counts.is_finite() {
            eyre::bail!("calibration produced invalid tare baseline");
        }
        let offset_i32 = zero_counts.round() as i32;

        Ok(Calibration {
            offset: offset_i32,
            scale_factor: a as f32,
        })
    }
}

/// Perform a single-step robust refit by rejecting outliers defined by |residual| > k * rms
/// around the initial line y = a0*x + b0. Uses an online (Welford/Chan) covariance update
/// over inliers only to compute slope and intercept. Returns None when refit is not applicable
/// (e.g., non-finite/zero rms, <2 inliers, or degenerate variance), in which case the caller
/// should keep the original (a0, b0).
fn robust_refit(pts: &[(i64, f32)], a0: f64, b0: f64, rms: f64, k: f64) -> Option<(f64, f64)> {
    if !(rms.is_finite() && rms > 0.0 && k.is_finite() && k > 0.0) {
        return None;
    }
    let n_pts = pts.len();
    if n_pts < 2 {
        return None;
    }

    let thr = k * rms;
    // Online means and covariance accumulators for inliers
    let mut n_in: usize = 0;
    let mut mean_x = 0.0f64;
    let mut mean_y = 0.0f64;
    let mut cxx = 0.0f64;
    let mut cxy = 0.0f64;

    for (x_i, y_i) in pts.iter() {
        let x = *x_i as f64;
        let y = *y_i as f64;
        let r = y - (a0 * x + b0);
        if r.abs() <= thr {
            // Inlier: update online means and covariances
            let n_old = n_in as f64;
            n_in += 1;
            let n_new = n_in as f64;
            let dx = x - mean_x;
            let dy = y - mean_y;
            let mean_x_new = mean_x + dx / n_new;
            let mean_y_new = mean_y + dy / n_new;
            // Chan's update for covariance terms
            cxx += dx * (x - mean_x_new);
            cxy += dx * (y - mean_y_new);
            mean_x = mean_x_new;
            mean_y = mean_y_new;
            let _ = n_old; // silence unused in optimized builds
        }
    }

    if n_in >= 2 && n_in < n_pts {
        if !(cxx.is_finite()) || cxx == 0.0 {
            return None;
        }
        let a = cxy / cxx;
        if !a.is_finite() || a == 0.0 {
            return None;
        }
        let b = mean_y - a * mean_x;
        Some((a, b))
    } else {
        None
    }
}

// Ergonomic conversions for building Calibration
impl TryFrom<Vec<CalibrationRow>> for Calibration {
    type Error = eyre::Report;
    fn try_from(rows: Vec<CalibrationRow>) -> Result<Self, Self::Error> {
        Self::from_rows(rows)
    }
}

impl TryFrom<&[CalibrationRow]> for Calibration {
    type Error = eyre::Report;
    fn try_from(rows: &[CalibrationRow]) -> Result<Self, Self::Error> {
        Self::from_rows(rows.to_vec())
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

    Calibration::try_from(rows)
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
        if self.safety.no_progress_epsilon_g <= 0.0 || self.safety.no_progress_epsilon_g > 1.0 {
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
        if let Some(alpha) = self.filter.ema_alpha
            && !(alpha > 0.0 && alpha <= 1.0)
        {
            eyre::bail!("filter.ema_alpha must be in (0.0, 1.0]");
        }

        // Predictor
        if self.predictor.window == 0 {
            eyre::bail!("predictor.window must be >= 1");
        }
        if self.predictor.min_progress_ratio < 0.0 || self.predictor.min_progress_ratio > 1.0 {
            eyre::bail!("predictor.min_progress_ratio must be in [0.0, 1.0]");
        }

        // Timeouts
        if self.timeouts.sample_ms == 0 {
            eyre::bail!("timeouts.sample_ms must be >= 1");
        }

        // Hardware
        if self.hardware.sensor_read_timeout_ms == 0 {
            eyre::bail!("hardware.sensor_read_timeout_ms must be >= 1");
        }

        // E-stop
        if self.estop.debounce_n == 0 {
            eyre::bail!("estop.debounce_n must be >= 1");
        }
        if self.estop.poll_ms == 0 {
            eyre::bail!("estop.poll_ms must be >= 1");
        }

        // Runner: no extra validation; serde restricts to known modes

        Ok(())
    }
}

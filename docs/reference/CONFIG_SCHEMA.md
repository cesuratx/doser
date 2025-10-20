# Configuration Schema

Typed TOML loaded by `doser_config`. This document describes all fields, defaults, and validation rules enforced by `Config::validate()`.

Example minimal file: see `etc/doser_config.toml`.

## Table of Contents

- [pins](#pins)
- [filter](#filter)
- [control](#control)
- [timeouts](#timeouts)
- [safety](#safety)
- [logging](#logging)
- [hardware](#hardware)
- [calibration CSV](#calibration-csv)
- [predictor](#predictor)

## [pins]

- hx711_dt: u8 (required)
- hx711_sck: u8 (required)
- motor_step: u8 (required)
- motor_dir: u8 (required)
- motor_en: u8 (optional, active-low enable)
- estop_in: u8 (optional, active-low E‑stop input)

## [filter]

- ma_window: usize (>= 1). Default: 1
- median_window: usize (>= 1). Default: 1
- sample_rate_hz: u32 (> 0). Default: 50

## [control]

- coarse_speed: u32 (> 0). Default: 1200
- fine_speed: u32 (> 0). Default: 250
- slow_at_g: f32 (>= 0). Default: 1.0
- hysteresis_g: f32 (>= 0). Default: 0.07
- stable_ms: u64 (<= 300_000). Default: 250
- epsilon_g: f32 ([0.0, 1.0]). Default: 0.08

Semantics:

- Coarse/fine speed selection by error magnitude vs `slow_at_g`.
- Completion uses asymmetric stop threshold `w + epsilon_g >= target`.
- Stability (settle) requires `|err| <= hysteresis_g` for `stable_ms` after stopping.

## [timeouts]

- sample_ms: u64 (>= 1). Default: 150

## [safety]

- max_run_ms: u64 (>= 0). Default: 60_000 (when not provided in config)
- max_overshoot_g: f32 (>= 0). Default: 2.0 (when not provided in config)
- no_progress_epsilon_g: f32 ((0.0, 1.0]). Default: 0.02
- no_progress_ms: u64 (>= 1, <= 86_400_000). Default: 1200

Semantics:

- E‑stop: debounced and latched until `begin()`.
- No‑progress watchdog: abort if weight change < epsilon for at least `no_progress_ms`.

## [logging]

- file: Option<String> path to a .log (JSON/pretty lines)
- rotation: Option<String> policy: "never" | "daily" | "hourly" (default: never)

Notes:

- Console log level via CLI `--log-level` or `RUST_LOG`; `[logging]` does not set level.
- When `file` is set, a non‑blocking file sink is added; a WorkerGuard is held for process lifetime.

## [hardware]

- sensor_read_timeout_ms: u64 (>= 1). Default: 150

## [predictor]

- enabled: bool. Default: false
- window: usize (>= 1). Default: 6
- extra_latency_ms: u64 (>= 0). Default: 20
- min_progress_ratio: f32 ([0.0, 1.0]). Default: 0.10

Semantics:

- When enabled, the core maintains a rolling slope estimate and predicts in-flight grams using the configured extra latency. If the predicted final mass (current + in-flight + epsilon) would cross target, the motor is stopped early to reduce overshoot. Activation is gated until at least `min_progress_ratio` of target is reached to avoid early noise.

## Calibration CSV

- Strict header: `raw,grams`
- At least 2 rows; raw values must be strictly monotonic (no duplicates, no zig‑zag)
- OLS fit across all rows computes `grams = a*raw + b`
- Produced calibration used by core as: `scale_factor = a`; `offset` is tare counts `round(-b/a)`

Outlier handling (robust refit):

- After the initial OLS fit, RMS residual is computed. Points with |residual| > 2×RMS are considered outliers and excluded from a one‑pass refit using numerically stable online covariance updates.
- If fewer than 2 inliers remain, or X variance is degenerate, the initial fit is kept.
- Zero slope (perfectly horizontal grams) is treated as invalid for calibration; raw must vary and map to varying grams.

# Doser Project

![CI](https://github.com/cesuratx/doser/actions/workflows/ci.yml/badge.svg)
![Security](https://github.com/cesuratx/doser/actions/workflows/security.yml/badge.svg)
![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)

## ⚠️ Important Notices

### API Stability

**This project is pre-1.0 and under active development.** The API may change significantly between minor versions (0.x releases). For production use, pin to exact versions:

```toml
doser_core = "=0.1.0"  # Exact version pinning recommended
```

**API Stability Policy:**

- **Pre-1.0 (current)**: Minor versions (0.x) may contain breaking changes. Patch versions (0.x.y) are backwards-compatible bug fixes only.
- **Post-1.0**: Follows strict semantic versioning. Breaking changes only in major versions (x.0.0). Deprecations announced one minor version in advance.

### Safety Notice

**This software is provided for educational and experimental use only.**  
It has **NOT been certified** for safety-critical applications.

For production dosing systems in commercial or safety-critical environments, you **MUST**:

- ✅ Perform independent safety analysis (FMEA, FTA)
- ✅ Implement redundant safety mechanisms (e.g., independent overfill detection)
- ✅ Comply with applicable regulations (FDA, CE, IEC 61508, ISO 13849, etc.)
- ✅ Obtain professional engineering review and certification
- ✅ Conduct thorough testing with your specific hardware and materials
- ✅ Implement proper fail-safe mechanisms and emergency stops

**⚠️ USE AT YOUR OWN RISK. NO WARRANTY PROVIDED.**

See LICENSE files for full legal terms.

---

## Quick Start

- Requires Rust (stable). On macOS/Linux:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

- Clone and run with simulated hardware (default, no GPIO required):

```bash
git clone https://github.com/cesuratx/doser.git
cd doser
# Use the provided typed config at ./doser_config.toml
# Simulation: set a small per-read increment so weight rises gradually
DOSER_TEST_SIM_INC=0.01 cargo run --release -p doser_cli -- --config ./doser_config.toml --log-level info dose --grams 10
```

Self-check (simulation backend):

```bash
cargo run --release -p doser_cli -- --config ./doser_config.toml self-check
```

Optional flags:

- --json to log as JSON lines
- --max-run-ms and --max-overshoot-g to override safety at runtime

### Simulation notes

- DOSER_TEST_SIM_INC controls how much the simulated weight increases on each read while the motor is running (e.g., 0.005–0.02).
- The simulator only increments while the motor runs; it stops increasing after the controller stops the motor.
- For more detail, add `--log-level debug` before the subcommand.

### Hardware Self-Check and Dose (Raspberry Pi)

Hardware support is feature-gated and intended for Raspberry Pi (Linux). On macOS, only simulation builds.

```bash
# On Raspberry Pi
cargo run --release -p doser_cli --features hardware -- \
  --config ./doser_config.toml self-check

# Run a real dose
cargo run --release -p doser_cli --features hardware -- \
  --config ./doser_config.toml dose --grams 5
```

Notes:

- If you have an enable (EN) pin on the stepper driver, set `pins.motor_en` in the TOML. EN is handled as active-low (low = enabled).
- An optional E‑stop input can be configured via `pins.estop_in` (active-low by default in the CLI wiring). E‑stop is debounced and latched until `begin()`.

#### Hardware Test Checklist

- Power off, wire per BCM pins in `doser_config.toml` (DT/SCK, STEP/DIR, optional EN, optional E‑stop).
- Secure the mechanism and keep an E‑stop path ready.
- Provide a calibration CSV for accurate grams; without it, defaults map 0.01 g/count (sim-friendly but not calibrated for hardware).
- Run self-check:
  - `cargo run --release -p doser_cli --features hardware -- --config ./doser_config.toml self-check`
  - Expect a successful scale read and a brief motor start/stop, then `OK`.
- Start with a small dose (1–2 g) and `--log-level info`:
  - `cargo run --release -p doser_cli --features hardware -- --config ./doser_config.toml dose --grams 2`
- Tune if needed:
  - Lower `control.fine_speed` and/or raise `control.epsilon_g` for softer finishes.
  - Verify safety: `safety.max_run_ms`, `safety.max_overshoot_g`, and no-progress settings are appropriate.

## Overview

Doser is a robust, safe dosing system with hardware abstraction and a simulation mode. Core features:

- Safety guards (max runtime, overshoot, no-progress watchdog, E‑stop debounce + latch)
- Calibration and tare (strict CSV header `raw,grams`; OLS fit across all rows)
- Median + moving-average filtering
- Hysteresis + settle time near target
- Typed TOML configuration + CLI overrides
- Hardware: HX711-backed scale and step/dir motor driver (feature-gated), plus simulation backends

Crates:

- doser_core: control loop, configs, errors
- doser_cli: CLI, config/CSV loading, logging
- doser_config: typed config/CSV loaders
- doser_hardware: hardware and simulation backends
- doser_traits: Scale/Motor traits and Clock

## Documentation

📖 **[Complete Documentation Index](./docs/INDEX.md)** - Start here for comprehensive navigation

**Quick Links**:

- **Getting Started**: [Developer Handbook](./docs/guides/DeveloperHandbook.md) | [Rust Primer](./docs/guides/RUST_PRIMER.md)
- **Configuration**: [Config Schema](./docs/reference/CONFIG_SCHEMA.md) | [Operations Guide](./docs/reference/OPERATIONS.md)
- **Architecture**: [System Overview](./docs/architecture/ARCHITECTURE.md) | [Concepts](./docs/concepts/)
- **Operations**: [Runbook](./docs/ops/Runbook.md) | [Pi Smoke Test](./docs/reference/PI_SMOKE.md)
- **Reviews**: [Security](./docs/reviews/security-performance-review.md) | [Business](./docs/reviews/business-best-practices-review.md)

**By Role**:

- 👤 **User**: [Config](./docs/reference/CONFIG_SCHEMA.md) | [Operations](./docs/reference/OPERATIONS.md)
- 👨‍💻 **Developer**: [Handbook](./docs/guides/DeveloperHandbook.md) | [Architecture](./docs/architecture/) | [ADRs](./docs/adr/)
- 🔧 **Operator**: [Runbook](./docs/ops/Runbook.md) | [Troubleshooting](./docs/reference/OPERATIONS.md)

## Configuration (TOML)

Default path used above: ./doser_config.toml

```toml
[pins]
# HX711 pins
hx711_dt = 5
hx711_sck = 6
# Stepper pins
motor_step = 13
motor_dir = 19
# Optional enable (active-low)
motor_en = 21
# Optional E-Stop input (active-low by default; configurable below)
estop_in = 26

[filter]
ma_window = 4
median_window = 3
sample_rate_hz = 50

[control]
coarse_speed = 1200
fine_speed = 250
slow_at_g = 1.0
hysteresis_g = 0.05
stable_ms = 250
epsilon_g = 0.02

[timeouts]
sample_ms = 100

[safety]
max_run_ms = 60000
max_overshoot_g = 2.0
# abort if weight doesn't change by ≥ epsilon within this window
no_progress_epsilon_g = 0.02
no_progress_ms = 1200

[logging]
file = "doser.log"
# Log rotation policy: "never" | "daily" | "hourly"
rotation = "never"

# Optional hardware-specific settings
[hardware]
# Max time to wait for HX711 data-ready before returning a timeout
sensor_read_timeout_ms = 150

# Optional E‑stop configuration (used when pins.estop_in is set)
[estop]
active_low = true     # treat low level as pressed
debounce_n = 2        # consecutive polls required to latch
poll_ms = 5           # polling interval for GPIO-backed checker

# Runner/orchestration defaults: "sampler" (default) or "direct"
[runner]
mode = "sampler"
```

Notes:

- Missing [safety] values fall back to safe defaults; CLI flags take precedence.
- no_progress_ms must be >= 1 (0 is invalid).
- Console log level is controlled by the CLI flag `--log-level` or `RUST_LOG`. The `[logging]` section configures only the optional file sink (`file`, `rotation`).
- On hardware builds, sampling is event-driven using HX711 DRDY; in simulation, sampling is paced by `filter.sample_rate_hz`.

## Precision tuning

- For tighter finishes in simulation and hardware, start with:

```toml
[control]
slow_at_g = 2.0
fine_speed = 90
epsilon_g = 0.05
hysteresis_g = 0.06
stable_ms = 500
```

- In simulation, use a smaller increment for a finer approach:
  - zsh: `DOSER_TEST_SIM_INC=0.005 cargo run -p doser_cli -- --config ./doser_config.toml --log-level debug dose --grams 10`
- For hardware, provide a calibration CSV and then fine-tune `fine_speed` and `epsilon_g` to your mechanism’s inertia.

## Calibration (CSV)

Note: The calibration CSV is optional. If you don’t pass --calibration, defaults are used (zero_counts=0, gain=0.01), which matches the simulator’s 0.01 g/count output but yields uncalibrated readings on real hardware. For accurate hardware dosing, supply a calibration CSV.

Provide a strict CSV with the exact headers:

```csv
raw,grams
842913,0.0
1024913,100.0
```

- At least 2 rows required; raw values must be strictly monotonic (no duplicates, no zig-zag).
- An ordinary least squares fit computes grams = a\*raw + b across all rows. The core uses `scale_factor=a` and `offset` as tare counts.

Use with the CLI:

```bash
cargo run --release -p doser_cli -- --config ./doser_config.toml dose --grams 18.5 \
  --calibration ./calibration.csv
```

## Logging and Tracing

- Console: pretty or JSON (`--json`).
- File: when `logging.file` is set in the TOML, a non-blocking appender writes in parallel to the file. The writer is kept alive for process lifetime.
- Rotation: choose `never` (default), `daily`, or `hourly` via `logging.rotation`.
- Trace control decisions: run with `--log-level trace` or set `RUST_LOG=trace`.

## Deterministic time in tests

The core exposes a `Clock` trait with monotonic time and helpers: `now() -> Instant`, `sleep(Duration)`, and `ms_since(epoch: Instant) -> u64`. Tests inject a deterministic clock via `DoserBuilder::with_clock(...)` to advance time without sleeping. The default real clock is `MonotonicClock`; tests can use a deterministic `TestClock`.

Type‑checked builder: The core uses a type‑state builder so `build()` is only available after providing scale, motor, and target grams. Typical usage remains simple:

```rust
let mut doser = doser_core::Doser::builder()
    .with_scale(my_scale)
    .with_motor(my_motor)
    .with_filter(my_filter)
    .with_control(my_control)
    .with_timeouts(my_timeouts)
    .with_target_grams(18.5)
    .build()?;
```

## Hardware Feature

Simulation (no hardware) is the default. To enable real GPIO/HX711 and motor control on Raspberry Pi builds:

```bash
cargo run --release -p doser_cli --features hardware -- --config ./doser_config.toml --grams 18.5
```

Under the hood:

- HardwareScale wraps the HX711 driver and performs timed reads.
- HardwareMotor runs a background thread toggling the STEP pin up to ~5 kHz, with optional active-low EN control.
- `make_estop_checker` provides a polled GPIO-backed E‑stop closure.

## Testing

- Unit tests for core logic use simulated hardware and deterministic clocks (`rstest`).
- CLI integration tests use `assert_cmd` and read operator messages from stderr.

Run all tests:

```bash
cargo test
```

## License

MIT

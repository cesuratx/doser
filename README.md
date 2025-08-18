# Doser Project

![CI](https://github.com/cesuratx/doser/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)

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
cargo run --release -p doser_cli -- --config ./doser_config.toml --grams 18.5
```

Optional flags:

- --json to log as JSON lines
- --max-run-ms and --max-overshoot-g to override safety at runtime

### Hardware Self-Check and Dose (Raspberry Pi)

Hardware support is feature-gated and intended for Raspberry Pi (Linux). On macOS, only simulation builds.

```bash
# On Raspberry Pi
cargo run --release -p doser_cli --features hardware -- \
  --config ./doser_config.toml SelfCheck

# Run a real dose
cargo run --release -p doser_cli --features hardware -- \
  --config ./doser_config.toml Dose --grams 5
```

Notes:

- If you have an enable (EN) pin on the stepper driver, set `pins.motor_en` in the TOML or export `DOSER_EN_PIN` in the environment. EN is handled as active-low (low = enabled).
- An optional E‑stop input can be configured via `pins.estop_in` (active-low by default in the CLI wiring).

## Overview

Doser is a robust, safe dosing system with hardware abstraction and a simulation mode. Core features:

- Safety guards (max runtime, overshoot, no-progress watchdog, E‑stop latch)
- Calibration and tare
- Median + moving-average filtering
- Hysteresis + settle time near target
- Typed TOML configuration + CLI overrides
- Hardware: HX711-backed scale and step/dir motor driver (feature-gated), plus simulation backends

Crates:

- doser_core: control loop, configs, errors
- doser_cli: CLI, config/CSV loading, logging
- doser_config: typed config/CSV loaders
- doser_hardware: hardware and simulation backends
- doser_traits: Scale/Motor traits

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
# Optional enable (active-low); if omitted, can also set env DOSER_EN_PIN
motor_en = 21
# Optional E-Stop input (active-low)
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

[timeouts]
sample_ms = 100

[safety]
max_run_ms = 60000
max_overshoot_g = 2.0
# Abort if weight change < epsilon for at least this many ms (0 disables)
no_progress_epsilon_g = 0.05
no_progress_ms = 2000

[logging]
file = "doser.log"
level = "info"
# Log rotation policy: "never" | "daily" | "hourly"
rotation = "never"
```

Notes:

- Missing [safety] values fall back to safe defaults; CLI flags take precedence.
- On macOS, the hardware feature will not compile; build hardware on Raspberry Pi with `--features hardware`.

## Calibration (CSV)

Provide a simple CSV with headers and key/value rows:

```csv
kind,key,value
scale,offset,0
scale,scale_factor,1.0
```

Use with the CLI:

```bash
cargo run --release -p doser_cli -- --config ./doser_config.toml --grams 18.5 \
  --calibration ./calibration.csv
```

- offset is the tare baseline in raw counts
- scale_factor is grams per count (gain)

## Logging

- Console: pretty or JSON (`--json`).
- File: when `logging.file` is set in the TOML, a non-blocking log appender writes JSON/pretty entries to that file in addition to the console. The log writer is kept alive for process lifetime.
- Rotation: choose `never` (default), `daily`, or `hourly` via `logging.rotation`.

## Deterministic time in tests

The core exposes a `Clock` trait (`fn now_ms() -> u64`). Tests inject a deterministic clock via `DoserBuilder::with_clock(...)` to advance time without sleeping.

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

Run the full workspace tests (simulation only):

```bash
cargo test
```

## License

MIT

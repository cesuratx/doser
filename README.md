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
# An example typed config is in etc/doser_config.toml
cargo run --release --bin doser_cli -- --grams 18.5
```

Optional flags:

- --json to log as JSON lines
- --max-run-ms and --max-overshoot-g to override safety at runtime

## Overview

Doser is a robust, safe dosing system with hardware abstraction and a simulation mode. Core features:

- Safety guards (max runtime, overshoot)
- Calibration and tare
- Median + moving-average filtering
- Hysteresis + settle time near target
- Typed TOML configuration + CLI overrides

Crates:

- doser_core: control loop, configs, errors
- doser_cli: CLI, config/CSV loading, logging
- doser_config: typed config/CSV loaders
- doser_hardware: hardware and simulation backends
- doser_traits: Scale/Motor traits

## Configuration (TOML)

Default path: etc/doser_config.toml (the CLI uses this by default; override with --config FILE)

```toml
[pins]
# HX711 pins
hx711_dt = 5
hx711_sck = 6
# Stepper pins
motor_step = 13
motor_dir = 19
# Optional E-Stop input
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

[logging]
file = "doser.log"
level = "info"
```

Notes:

- Missing [safety] values fall back to safe defaults; CLI flags take precedence.
- On macOS, only the simulation backend is built. Hardware is feature-gated.

## Calibration (CSV)

Provide a simple CSV with headers and key/value rows:

```csv
kind,key,value
scale,offset,0
scale,scale_factor,1.0
```

Use with the CLI:

```bash
cargo run --release --bin doser_cli -- --grams 18.5 --calibration etc/calibration.csv
```

- offset is the tare baseline in raw counts
- scale_factor is grams per count (gain)

## Hardware Feature

Simulation (no hardware) is the default. To enable real GPIO/I2C on Raspberry Pi builds:

```bash
cargo run --release --bin doser_cli --features hardware -- --grams 18.5
```

## Testing

Run the full workspace tests (simulation only):

```bash
cargo test
```

## License

MIT

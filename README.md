# Doser Project

![CI](https://github.com/cesuratx/doser/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)

<!-- Add crates.io and docs.rs badges if published -->

## Quick Start

Install Rust, clone the repo, and run a simulated dosing session:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/cesuratx/doser.git
cd doser
cargo run --release --bin doser_cli -- --grams 18.5
```

See `examples/quick_start.rs` for a minimal code example.

## Overview

Doser is a robust, memory-safe dosing system for Raspberry Pi and similar hardware, written in idiomatic Rust. It provides precise control of dosing operations (e.g., bean dosing, powder dispensing) with hardware abstraction, simulation, advanced calibration, and flexible configuration.

## Supported Hardware

- Raspberry Pi (all models)
- HX711 load cell amplifier
- Stepper motor drivers (A4988, DRV8825, TMC series)
- Any scale or motor supported via trait implementation

## Architecture

- **doser_core**: Core logic, dosing algorithms, builder pattern, error types, trait-based config and calibration, dosing strategies, progress bar, logging.
- **doser_cli**: Command-line interface (CLI) for user interaction, config/CLI integration, dosing loop, calibration, logging, hardware/simulation switching.
- **doser_hardware**: Hardware abstraction (GPIO, I2C, SPI, PWM) via safe Rust traits. Supports both real hardware and simulation.
- **doser_ui**: (Optional/future) UI module for graphical/web control.

## Feature Flags

- `hardware`: Enables real hardware access (GPIO, I2C, SPI, PWM)
- Default: simulation mode (no hardware required)

## Key Features

- **Trait-based Hardware Abstraction**: Interact with real or simulated hardware using safe Rust traits (`Scale`, `Motor`).
- **Dosing Strategy**: Pluggable dosing algorithms, including adaptive motor control and moving average filtering for precision.
- **Flexible Configuration**: Load config from TOML/CSV files, override via CLI. Trait-based config and calibration sources.
- **Advanced Calibration**: Multi-point calibration, average scale factor calculation, and diagnostics logging.
- **Builder Pattern**: Ergonomic, safe setup of dosing sessions.
- **Logging**: Structured logging via trait, with dosing results and calibration data.
- **Memory Safety**: No unsafe code in your logic; all hardware access is via safe Rust APIs (rppal crate internally uses unsafe, but exposes safe API).
- **Simulation Support**: Switch between hardware and simulation with feature flags for easy testing and development.

## Troubleshooting

- **Permissions:** Ensure your user has access to GPIO/I2C/SPI devices on Raspberry Pi.
- **Config errors:** Double-check TOML/CSV config files and CLI arguments.
- **Hardware issues:** Use simulation mode to isolate software bugs.
- **Logs:** See `dosing_log.txt` for diagnostics.

## Safety

- All code is written in safe Rust. No `unsafe` blocks in your logic or abstractions.
- Hardware access is encapsulated in the `doser_hardware` crate, using safe APIs from `rppal`.
- Dynamic dispatch (`Box<dyn Trait>`) and mutable references are used safely and idiomatically.

## Installation

### Prerequisites

- **Rust**: Install Rust with:

  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

- **Raspberry Pi Hardware Support**: For real hardware, enable the `hardware` feature and ensure you have the required GPIO/I2C/SPI libraries and permissions.

## Examples

- [Quick Start](examples/quick_start.rs)
- [Custom Dosing Strategy](examples/custom_strategy.rs)
- [Simulated Hardware](examples/simulated_hardware.rs)

## Usage

Build and run the CLI:

```bash
cargo run --release --bin doser_cli -- --grams 18.5 --dt-pin 5 --sck-pin 6 --step-pin 13 --dir-pin 19
```

Calibrate the scale:

```bash
cargo run --release --bin doser_cli -- --calibrate 100.0 --dt-pin 5 --sck-pin 6
```

Configuration can be provided via TOML or CSV files (`doser_config.toml`, `doser_config.csv`). CLI arguments override config file values.

Example TOML config:

```toml
hx711_dt_pin = 5
hx711_sck_pin = 6
stepper_step_pin = 13
stepper_dir_pin = 19
max_attempts = 100
```

Example CSV config:

```csv
hx711_dt_pin,5
hx711_sck_pin,6
stepper_step_pin,13
stepper_dir_pin,19
max_attempts,100
```

### Feature Flags

To run in simulation mode (no hardware required), use default build. To enable hardware, build with:

```bash
cargo run --release --bin doser_cli --features hardware -- --grams 18.5
```

### Logging

Dosing results and calibration data are logged to `dosing_log.txt` in the project directory.

## How It Works

1. **Hardware Abstraction**: The CLI selects hardware or simulation at runtime using feature flags. All hardware access is via safe trait methods (`read_weight`, `start`, `stop`).
2. **Session Setup**: The builder pattern collects all config/CLI values and constructs a validated dosing session.
3. **Calibration**: Multi-point calibration data is loaded and used to calculate an average scale factor for precision.
4. **Dosing Loop**: The dosing strategy uses a moving average filter for scale readings and adaptive motor control (slows motor near target) for accuracy.
5. **Logging**: All dosing results and calibration diagnostics are logged for traceability.

## API Documentation

- If published, see [docs.rs](https://docs.rs/doser_core) for API docs.

## Extensibility

- Add new dosing strategies by implementing the `DosingStrategy` trait.
- Add new hardware types by implementing the `Scale` and `Motor` traits.
- Extend config/calibration sources via trait-based abstraction.

## Safety and Best Practices

- No unsafe code in your logic; all hardware access is via safe Rust APIs.
- All hardware interaction is encapsulated and testable via simulation.
- Builder pattern and trait-based design ensure maintainability and extensibility.

## License

MIT

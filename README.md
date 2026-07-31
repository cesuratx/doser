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

Self-check — measures the scale's sample rate (simulation backend):

```bash
cargo run --release -p doser_cli -- --config ./doser_config.toml self-check
# → Detected HX711 rate: 80 SPS
```

Health check — proves the scale reads and the motor starts/stops:

```bash
cargo run --release -p doser_cli -- --config ./doser_config.toml health
# → ✓ Scale: responsive (raw: 0)
#   ✓ Motor: responsive
#   Health check: OK
```

Optional flags:

- `--json` (global, before the subcommand) to log as JSON lines and print a JSONL result line
- `--max-run-ms` and `--max-overshoot-g` (flags of `dose`) to override safety at runtime

### Output streams

- **stdout** carries only the CLI's own output: the `--json` result line, `final: X.XX g`,
  `Detected HX711 rate: …`, and the monitor/jog status lines.
- **stderr** carries every log record (pretty *and* JSON) plus `--stats` output and errors.
  So `doser_cli --json … dose --grams 5 > run.jsonl` yields exactly one JSON line.

### Simulation notes

- DOSER_TEST_SIM_INC controls how much the simulated weight increases on each read while the motor is running (e.g., 0.005–0.02).
- The simulator only increments while the motor runs; it stops increasing after the controller stops the motor.
- For more detail, add `--log-level debug` before the subcommand.

### Hardware Self-Check and Dose (Raspberry Pi)

Hardware support is feature-gated and intended for Raspberry Pi (Linux). On macOS, only simulation builds.

```bash
# On Raspberry Pi — scale + motor reachable?
cargo run --release -p doser_cli --features hardware -- \
  --config ./doser_config.toml health

# What sample rate is the HX711 actually running at?
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
- Verify the two halves of the machine separately:
  - `cargo run --release -p doser_cli --features hardware -- --config ./doser_config.toml health`
    — reads the scale once and briefly starts/stops the motor. Expect
    `✓ Scale: responsive (raw: …)`, `✓ Motor: responsive`, then `Health check: OK`.
    This is the check that touches the motor.
  - `cargo run --release -p doser_cli --features hardware -- --config ./doser_config.toml self-check`
    — reads the scale for one second and prints `Detected HX711 rate: {10|80} SPS`. It never
    touches the motor and never prints `OK`. Treat the rate as a hint only: the detector
    labels any inter-read gap under 50 ms as 80 SPS, so a desynced read looks healthy
    (see docs/ops/HARDWARE_LESSONS.md, Lesson 4).
- Start with a small dose (1–2 g) and `--log-level info`:
  - `cargo run --release -p doser_cli --features hardware -- --config ./doser_config.toml dose --grams 2`
- Tune if needed:
  - Lower `control.fine_speed` and/or raise `control.epsilon_g` for softer finishes.
  - Verify safety: `safety.max_run_ms`, `safety.max_overshoot_g`, and no-progress settings are appropriate.

## Bench tools: live monitor and motor jog

Two subcommands exist for bring-up and bench work rather than for dosing. Both work with the
simulation backend too, so you can try them on a laptop.

### `monitor` — live weight web UI

Serves a small self-contained page (no CDN, works offline on the Pi) that polls the current
scale reading ~10×/s and lets you tare from the browser. Useful for watching the raw counts
while you press on the load cell, and for collecting calibration points.

```bash
# Recommended: keep it on the Pi and reach it over an SSH tunnel
cargo run --release -p doser_cli --features hardware -- \
  --config ./etc/doser_config.toml monitor --bind 127.0.0.1 --port 8080
# then, from your laptop:  ssh -L 8080:127.0.0.1:8080 pi@doser.local
```

- `--bind` defaults to **0.0.0.0**, which publishes the UI on every interface. **The server
  has no authentication and no TLS**, so anyone who can route to the machine can watch the
  scale and tare it. The CLI prints a `WARNING:` line on stderr whenever the bind address is
  not loopback. Use `--bind 127.0.0.1` unless you trust the whole network.
- `--hz` overrides the sample rate; it defaults to `filter.sample_rate_hz` from the config.
- Endpoints: `GET /` (page), `GET /reading` (JSON), `POST /tare`, `POST /tare/clear`.
- **The two POST endpoints require the custom header `X-Doser-Monitor`.** It makes any
  cross-origin POST non-"simple", so a browser must preflight it and the preflight fails
  (the server sends no CORS headers) — that is what stops a random web page from taring your
  scale mid-dose. The monitor's own page sends the header; scripting the endpoint by hand
  does not, so you must add it:

  ```bash
  curl -X POST -H 'X-Doser-Monitor: 1' http://127.0.0.1:8080/tare
  ```

  Requests without the header get `403`. A tare before the first successful sample gets `409`
  ("no reading yet — check the wiring"). The POSTs additionally require a `Host` that names a
  LAN address (IP literal, single-label name, or a `.local`/`.lan`/`.internal`/`.home.arpa`
  suffix); reads are not gated at all.

### `motor` — fixed-rate jog

Spins the motor at a commanded rate for a bounded time with no scale and no control loop.
This is the tool for checking wiring, direction, and current limit during bring-up.

```bash
# 400 steps/s clockwise for 2 s
cargo run --release -p doser_cli --features hardware -- \
  --config ./etc/doser_config.toml motor --sps 400 --ms 2000

# roughly 800 steps counterclockwise
cargo run --release -p doser_cli --features hardware -- \
  --config ./etc/doser_config.toml motor --sps 400 --steps 800 --dir ccw
```

- `--sps` is range-checked by clap to `1..=5000` (`doser_hardware::MAX_STEP_RATE_SPS`, the
  rate both motor backends clamp to). Out of range is rejected up front with exit code 2:
  `error: invalid value '20000' for '--sps <HZ>': 20000 is not in 1..=5000`.
- `--ms` (default 1000) sets the run time; `--steps` overrides it and is **approximate** —
  it is converted to `ceil(steps / sps)` seconds and paced by the stepping thread, so
  scheduling jitter can land a step or two either side of the count.
- `--dir cw|ccw` drives the DIR line high/low.
- Ctrl-C stops the motor promptly; the driver is de-energized (active-low EN) as the motor is
  dropped, even if the explicit stop reports an error.

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

Two configs ship with the repo:

- **`etc/doser_config.toml`** — what `--config` defaults to. Tuned for the bench Pi's 10 SPS
  HX711 board (`sample_rate_hz = 10`, `sensor_ms = 200`, `stable_ms = 800`).
- **`./doser_config.toml`** — the precision example used throughout this README; assumes an
  80 SPS board.

`[pins]`, `[filter]` and `[timeouts]` are **required** — omitting one is a parse error, not a
fallback. Everything else is optional and defaulted. The full key-by-key reference (real
defaults, validation ranges, deprecated keys) lives in
[docs/reference/CONFIG_SCHEMA.md](./docs/reference/CONFIG_SCHEMA.md).

```toml
[pins]
# HX711 pins
hx711_dt = 5
hx711_sck = 6
# Stepper pins
motor_step = 23
motor_dir = 24
# Optional enable (active-low)
motor_en = 25
# Optional E-Stop input (active-low by default; configurable below)
estop_in = 12

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

- Missing `[safety]` values fall back to safe defaults: `doser_config` defaults `max_run_ms`
  and `max_overshoot_g` to 0, and the CLI treats 0 as "unset" and substitutes 60000 ms and
  2.0 g. `dose --max-run-ms` / `--max-overshoot-g` take precedence over both.
- `no_progress_ms` must be >= 1 (0 is invalid). It counts only while the motor is commanded
  to run.
- Console log level is controlled by the CLI flag `--log-level` or `RUST_LOG` (`RUST_LOG`
  wins). The `[logging]` section configures only the optional file sink (`file`,
  `rotation`); `[logging] level` is parsed and ignored.
- `timeouts.settle_ms` is parsed and **ignored** — it is a compatibility no-op. The settle
  window is `control.stable_ms`.
- `timeouts.sample_ms` also accepts the alias `sensor_ms`.
- A persisted `[calibration]` table (`gain_g_per_count`, `zero_counts`, optional `offset_g`)
  takes precedence over `--calibration <CSV>` at runtime.
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

Note: The calibration CSV is optional. If you don’t pass `--calibration` and the config has
no `[calibration]` table, defaults are used (`zero_counts = 0`, `gain = 0.01`), which matches
the simulator’s 0.01 g/count output but yields uncalibrated readings on real hardware. For
accurate hardware dosing, supply a calibration CSV or a persisted `[calibration]` table — the
table wins when both are present.

Provide a strict CSV with the exact headers:

```csv
raw,grams
842913,0.0
1024913,100.0
```

- At least 2 rows required; raw values must be strictly monotonic (no duplicates, no zig-zag);
  at most 100,000 rows.
- **No comments.** The reader has no comment character, so a `#` line is parsed as data (or
  as the header) and the file is rejected. Header plus rows only.
- An ordinary least squares fit computes grams = a\*raw + b across all rows, with a one-pass
  robust refit that drops points more than 2×RMS off the line. The core uses `scale_factor=a`
  and `offset` as tare counts.
- `doser_config.csv` in the repo root is a working example.

Use with the CLI — `--calibration` is a **global** flag, so it goes before the subcommand:

```bash
cargo run --release -p doser_cli -- --config ./doser_config.toml \
  --calibration ./calibration.csv dose --grams 18.5
```

## Logging and Tracing

- Console: pretty or JSON (`--json`) — **both go to stderr**. Redirect stderr to capture logs
  (`2> logs.jsonl`); stdout carries only the result/status lines.
- File: when `logging.file` is set in the TOML, a non-blocking appender writes in parallel to the file. The writer is kept alive for process lifetime.
- Rotation: choose `never` (default), `daily`, or `hourly` via `logging.rotation`.
- Trace control decisions: run with `--log-level trace` or set `RUST_LOG=trace`. `RUST_LOG`
  takes precedence over `--log-level` when it is set.

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
cargo run --release -p doser_cli --features hardware -- \
  --config ./doser_config.toml dose --grams 18.5
```

`--grams` belongs to the `dose` subcommand, so the subcommand name is not optional;
`-- --config … --grams 18.5` is rejected by clap with `unexpected argument '--grams'`.
Global flags (`--config`, `--calibration`, `--json`, `--log-level`) go **before** the
subcommand; everything else goes after it.

Under the hood:

- HardwareScale wraps the HX711 driver and performs timed reads.
- HardwareMotor runs a background thread toggling the STEP pin, clamped to
  `doser_hardware::MAX_STEP_RATE_SPS` (5000 sps), with optional active-low EN control.
- `make_estop_checker` provides a polled GPIO-backed E‑stop closure.

## Testing

- Unit tests for core logic use simulated hardware and deterministic clocks (`rstest`).
- CLI integration tests use `assert_cmd` and read operator messages from stderr (all log
  records are on stderr; only result lines are on stdout).
- The cargo-fuzz target (`fuzz/`) and the Criterion bench (`doser_core/benches/predictor.rs`)
  are **local, manual** steps — no CI job runs them. See
  [docs/testing/Strategy.md](./docs/testing/Strategy.md).

Runnable examples:

```bash
cargo run -p doser_cli --example quick_start
cargo run -p doser_cli --example simulated_hardware
cargo run -p doser_cli --example custom_strategy
```

Run all tests:

```bash
cargo test
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](./LICENSE-APACHE))
- MIT license ([LICENSE-MIT](./LICENSE-MIT))

at your option — the `MIT OR Apache-2.0` SPDX expression declared in `Cargo.toml`.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this work by you shall be dual licensed as above, without any additional
terms or conditions.

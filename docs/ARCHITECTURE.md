# Doser: Architecture and Rationale

This document explains what the system does, the safety/business requirements, and how the Rust codebase is organized to meet them. It’s written for newcomers to Rust and this project. Skim the overview first, then drill into modules as needed.

---

## 1. Business Problem and Safety Requirements

We are building a dosing controller for granular material (e.g., coffee beans). The controller turns a motor on/off (with speeds) and reads a scale sensor to approach a target weight. Key business and safety goals:

- Accuracy: Reach the target weight with minimal overshoot.
- Repeatability: Similar outcomes across runs and environments.
- Responsiveness: Stop quickly when reaching the target or on emergency stop.
- Safety: Abort on faults (E‑stop pressed, sensor timeouts, no progress, overshoot, max runtime).
- Operability: Clear logs, health checks, and configuration.
- Determinism in tests: Simulate hardware and control time for predictable unit tests.

These goals drive most of the design decisions below.

---

## 2. Workspace Overview

The repository is a Rust workspace of multiple crates (packages). Each crate has a focused responsibility, creating clear boundaries.

- `doser_traits`: Defines hardware-agnostic traits:

  - `Scale`: read weight data with a timeout.
  - `Motor`: start, set_speed, and stop.
    These traits allow swapping real hardware and simulators without changing core logic.

- `doser_core`: The heart of the dosing algorithm and safety logic. It depends only on traits, not on specific hardware.

  - Implements filtering (median + moving average) to smooth sensor noise.
  - Calibration utilities to convert raw counts to grams.
  - Safety guards: max runtime, overshoot, no-progress watchdog, emergency stop latch.
  - Deterministic `Clock` abstraction for tests.
  - Strongly-typed errors with consistent mapping from hardware errors.

- `doser_hardware`: Hardware backends.

  - `feature = "hardware"` enables Raspberry Pi GPIO implementations.
  - On non-ARM builds (e.g., CI on x86), rppal is not pulled in; hardware code is target-gated.
  - Provides simulated components when hardware feature is off.

- `doser_config`: Typed configuration, serde-based loading, and calibration CSV parsing.

- `doser_cli`: Command-line tool wiring everything together.

  - Logging setup (console/file, JSON/pretty, rotation).
  - Subcommands: `SelfCheck` and `Dose`.
  - Translates typed config into `doser_core` settings and runs a dose.

- `doser_ui`: Placeholder for future UI concerns.

- `examples/`: Quick-start samples and simulated hardware demos.

---

## 3. Control Loop Design (doser_core)

Core concepts used by the control loop:

- Target tracking: We measure weight repeatedly and drive the motor until we reach >= target.
- Two-speed approach: coarse (fast) until near target, then fine (slow) for precision.
- Settling time: Once within band, require stable milliseconds before declaring complete to avoid false positives from noise.
- Safety checks at each step:
  1. Emergency stop (E‑stop) latch: once tripped, remains latched until next `begin()`.
  2. Max runtime: abort if total time exceeds configured limit.
  3. Overshoot: abort if weight exceeds target by too much.
  4. No-progress watchdog: abort if weight hasn’t improved for N ms by at least epsilon grams.
- Motor lifecycle: start when needed, adjust speed, stop when done or on abort. Also ensure the motor is stopped in `Drop` to be safe.

Why this design?

- Granular dosing overshoots easily; two-speed and settle windows improve final accuracy.
- Latching E‑stop ensures safety dominance over transient reads.
- Watchdogs prevent stuck hardware from running forever.

---

## 4. Filtering and Calibration

- Median filter: suppresses spikes/outliers in samples.
- Moving average: reduces noise and provides smoother readings.
- Calibration:
  - Tare (zero) shifts baseline counts.
  - Gain/offset converts raw counts to grams.

Reasoning: Raw sensors (e.g., HX711) are noisy. Filtering and calibration produce stable, meaningful weights. We keep filtering simple (median + MA) for predictable behavior.

---

## 5. Deterministic Time via Clock Abstraction

Tests should not rely on real `std::time::Instant::now()` or `sleep()`. We define a `Clock` trait with `now_ms()`:

- `SystemClock` uses real time in production.
- `TestClock` is controlled by tests, advancing time programmatically to make behaviors deterministic and fast.

---

## 6. Error Handling and Mapping

We define a `DoserError` enum in `doser_core` as our single error surface, including:

- `Timeout` (e.g., sensor read timed out)
- `Hardware` and `HardwareFault` (non-timeout hardware failures)
- `Config` (invalid configuration)
- `State` (logic errors) and `Io`

Hardware-specific errors are mapped centrally to these variants, ensuring consistent behavior and simpler calling code. Typed mapping lets tests assert the right outcomes without string matching.

---

## 7. Emergency Stop (E‑stop)

- An optional function `Fn() -> bool` returns true if E‑stop is active.
- It is checked in each loop and latched (stays active) until a new `begin()` is called.
- Hardware factory `make_estop_checker` creates a GPIO-backed checker on ARM.
- On non-ARM or when not configured, a stub returns false.

Reasoning: E‑stop must be robust and always honored; latching prevents races between checks and motor control.

---

## 8. Configuration and Validation

`doser_config` provides typed structs for control, filtering, timeouts, and safety. The core builder validates ranges (e.g., window sizes > 0, sane hysteresis, nonnegative times) early and fails fast with `Config` errors if invalid.

Why? Bad configs cause confusing runtime behavior; fail early and clearly.

---

## 9. CLI and Logging

The CLI loads config and optional calibration CSV, initializes logging, and selects hardware vs. simulation based on features.

- `SelfCheck`: quick probe of scale read and motor start/stop. Prints `OK` or `*_ERROR: <msg>` for automation.
- `Dose`: builds a `Doser`, runs `step()` in a loop until complete or aborted, and prints a summary.

Logging:

- Pretty or JSON logs to console.
- Optional file logging with rotation (never/daily/hourly) and non-blocking writer to avoid backpressure.

---

## 10. Hardware Backends and Platform Gating

`doser_hardware` has two modes:

- Simulation (default, no `hardware` feature): in-memory fake scale/motor for tests and demos.
- Hardware (`--features hardware`): real GPIO/HX711/motor placeholders, plus an E‑stop checker.

Platform gating:

- `rppal` is included only when building on ARM targets.
- Hardware-only code blocks are behind `#[cfg(any(target_arch = "arm", target_arch = "aarch64"))]`.
- A stub E‑stop checker is provided on non-ARM to keep CI green while still compiling the feature.

---

## 11. Testing Strategy

- Unit tests in `doser_core/tests`: cover filtering, calibration, settling, safety guards, and E‑stop latch with a `TestClock`.
- Integration tests: exercise CLI help, self-check, logging init, and simulated error paths.
- CI runs fmt, clippy (deny warnings), and all tests in simulation mode. A separate job compiles hardware feature on x86 to validate gating.

Design tradeoffs:

- We do not run hardware tests in CI because there’s no device; instead we rely on platform guards and an optional future ARM CI job.

---

## 12. File Tour and Key APIs

- `doser_traits/src/lib.rs`:

  - `pub trait Scale { fn read(&mut self, timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> }`
  - `pub trait Motor { fn start(&mut self) -> Result<...>; fn set_speed(&mut self, u32) -> Result<...>; fn stop(&mut self) -> Result<...>; }`

- `doser_core/src/lib.rs` (high level):

  - `Doser::builder()` → chain `.with_scale()`, `.with_motor()`, `.with_filter()`, `.with_control()`, `.with_safety()`, `.with_timeouts()`, `.with_target_grams()`, optional `.with_estop_check()`, optional `.with_clock()`.
  - `Doser::begin()` resets internal state and E‑stop latch.
  - `Doser::step()` performs one control iteration, returning `DosingStatus::{Running, Complete, Aborted(DoserError)}`.
  - `Doser::last_weight()` returns the last filtered weight in grams.
  - `Doser::motor_stop()` stops the motor; `Drop` also tries to stop as a safeguard.

- `doser_hardware/src/lib.rs`:

  - Simulation types `SimulatedScale`, `SimulatedMotor` under `#[cfg(not(feature = "hardware"))]`.
  - Hardware stubs `HardwareScale`, `HardwareMotor`, and `make_estop_checker` under `#[cfg(feature = "hardware")]` with target-arch gating for rppal code.

- `doser_cli/src/main.rs`:
  - CLI parsing, config/calibration loading, logging init, hardware/sim selection, `SelfCheck`, and `Dose` loop.

---

## 13. Rust Concepts Used (with quick explanations)

- Workspaces: multiple crates managed together for modularity and fast builds.
- Features and cfg attributes: enable/disable code at compile time based on feature flags or target platform.
- Traits and generics: define behavior contracts (`Scale`, `Motor`) and write code generic over implementations.
- Trait objects (`Box<dyn Fn() -> bool + Send + Sync>`): runtime polymorphism for the E‑stop checker.
- Result and error handling: `Result<T, E>` with custom error types; `?` operator to bubble errors; mapping hardware errors to domain errors.
- Ownership and borrowing: components are moved into the `Doser` to ensure exclusive access during operation.
- Drop: automatic cleanup when a value goes out of scope—used to stop the motor defensively.
- Testing: unit and integration tests; deterministic time via a `Clock` trait to avoid sleeps.
- Logging: structured logs with `tracing`, composable layers, and non-blocking file writers.

---

## 14. What’s Next (Roadmap)

- Implement real HX711 and motor control using `rppal` on ARM devices; add debounce for E‑stop.
- Add an optional ARM CI job to build and run smoke tests on real hardware or an emulator.
- Enhance observability (metrics), packaging, and retention policies.
- Add advanced safety: jam/empty-hopper detection, stall detection, operator reset flow for latched faults.

---

## 15. How to Run

- Simulation mode (default features):

  - Build/tests: `cargo test --workspace --no-default-features`
  - CLI help: `cargo run -p doser_cli -- --help`
  - Self check: `cargo run -p doser_cli -- --config doser_config.toml SelfCheck`

- Hardware mode (on a Raspberry Pi/ARM):
  - `cargo run -p doser_cli --no-default-features --features hardware -- --config doser_config.toml Dose --grams 18`

Ensure pins and config match your wiring before running hardware mode.

---

If anything is unclear, open an issue or ask for a live walkthrough.

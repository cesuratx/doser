# Doser Architecture

This document describes the architecture of the Doser workspace, its crates, data flow, key Rust features used, and the safety invariants enforced by the core dosing logic.

## Workspace overview

The repository is a Rust workspace composed of multiple crates:

- doser_core: Hardware‑agnostic dosing logic (control loop, filtering, calibration, safety checks, state machine, builder).
- doser_traits: Thin abstraction layer defining hardware traits: Scale and Motor.
- doser_hardware: Hardware backends and a simulation backend. Includes an E‑stop checker factory for GPIO (feature‑gated) and simulated motor/scale. Provides:
  - HardwareScale wrapping an HX711 driver with timeout reads.
  - HardwareMotor (Raspberry Pi step/dir with optional active‑low EN pin) driven from a background thread up to ~5 kHz.
- doser_config: Typed configuration loader (TOML) and calibration CSV loader.
- doser_cli: CLI application that wires config, hardware or sim, and runs a dosing session. Initializes logging.
- doser_ui: Placeholder crate for higher‑level UI integrations.

Support files:

- examples/: Quick start and custom strategy examples.
- .github/workflows/: CI for fmt, clippy, tests, and hardware feature compile checks.

## High‑level data flow

```mermaid
flowchart LR
    subgraph User
      CMD[CLI args]
      CFGT[Config TOML]
      CALCSV[Calibration CSV]
    end

    CMD --> CLI["doser_cli"]
    CFGT --> CLI
    CALCSV --> CLI

    subgraph Runtime
      CLI -->|builds| BUILDER[Doser::builder()]
      BUILDER -->|with_scale/with_motor| HWSEL{Hardware?<br/>(feature flag)}
      HWSEL -->|hardware| HW[HardwareScale & HardwareMotor]
      HWSEL -->|simulation| SIM[SimulatedScale & SimulatedMotor]
      BUILDER -->|with_filter/with_control/with_safety/with_timeouts/with_calibration| DOSER[Doser]
    end

    HW --> DOSER
    SIM --> DOSER

    subgraph Loop
      DOSER -->|step()| STEP[step()]
      STEP --> SCALE[Scale::read(timeout)]
      SCALE --> FILTER[Median + Moving Avg]
      FILTER --> SAFETY[Safety Guards<br/>(max_run, overshoot, watchdog, E‑stop)]
      SAFETY --> STATUS{DosingStatus}
      DOSER --> MOTOR[Motor::start/set_speed/stop]
      STATUS --> CLI
    end

    CLI --> LOG[tracing: console + optional file]
    CLI --> OUT[stdout summary]
```

## Core (doser_core)

The `Doser` struct implements the dosing control loop. It is built via a `DoserBuilder`, which injects:

- Scale and Motor implementations (via `doser_traits`).
- FilterCfg, ControlCfg, SafetyCfg, Timeouts.
- Calibration (linear mapping counts→grams, plus tare counts).
- Optional E‑stop checker callback.
- Optional Clock (for deterministic time‑based tests).
- Type‑state builder: compile‑time enforcement that scale, motor, and target grams are provided before `build()` is available; other setters remain optional and order‑agnostic.

Important modules:

- lib.rs: Doser state machine, filtering, safety checks, builder API.
- error.rs: Workspace error types (e.g., `DoserError::{Config, Hardware, Timeout, State, Io}`).
- sampler.rs: Optional sampling helper (channel based).
- session.rs: Sample builder for sessions (range validation).
- tests/: Unit and integration tests for control, safety, calibration, error mapping, E‑stop, watchdog, and CLI integration.

### Control loop (step)

- Read scale with timeout -> raw counts.
- Convert to grams via calibration -> apply median prefilter and moving average.
- Update last weight; compute error to target.
- Safety checks in order:
  1. E‑stop callback (latched).
  2. Max runtime (>= `max_run_ms`).
  3. Overshoot guard if `w > target + max_overshoot_g`.
  4. No‑progress watchdog (threshold epsilon for at least `no_progress_ms`).
- Completion: asymmetric; when `w >= target` stop motor, start settle timer, require stability for `stable_ms` before returning `Complete`.
- Otherwise choose coarse/fine speed by error magnitude and command motor (ensure start before set_speed).

### Filtering

- Median prefilter (`median_window`) to reject spikes.
- Moving average (`ma_window`) to reduce noise.

### Time and determinism

- A `Clock` trait (`fn now_ms() -> u64`) is used instead of `Instant` for all time‑based logic (runtime cap, settle window, watchdog).
- `SystemClock` is the default. Tests can inject a `TestClock` via `DoserBuilder::with_clock(...)`.

### Safety invariants

- Motor is stopped immediately on E‑stop, on completion, and on any abort.
- Max runtime is a hard cap; overshoot and no‑progress guards protect product and hardware.
- Filter buffers never exceed configured window sizes.

## Traits (doser_traits)

- `trait Scale { fn read(&mut self, timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>>; }`
- `trait Motor { fn start(&mut self) -> Result<(), _>; fn set_speed(&mut self, s: u32) -> Result<(), _>; fn stop(&mut self) -> Result<(), _>; }`

These are implemented by simulation and (feature‑gated) hardware backends.

## Hardware (doser_hardware)

- Simulation: `SimulatedScale` and `SimulatedMotor` suitable for unit/integration tests.
- Hardware: `HardwareScale` wraps an `Hx711` driver; `HardwareMotor` drives step/dir pins on Raspberry Pi with an optional active‑low enable pin. A background thread toggles STEP; direction and enable are controlled from the main thread. Max step rate is clamped (~5 kHz).
- E‑stop: `make_estop_checker(pin, active_low, poll_ms)` creates a polled GPIO closure. The CLI wires it if `pins.estop_in` is set.
- Platform gating: hardware implementations are behind `--features hardware`; `rppal` is an optional dependency only compiled on Linux/ARM targets. On macOS, build uses simulation types.

## Config (doser_config)

- Typed TOML config (pins, filter, control, timeouts, safety, logging). New fields:
  - `pins.motor_en: Option<u8>` optional active‑low enable pin for the driver (or use `DOSER_EN_PIN`).
  - `logging.rotation: Option<String>` controls file rotation policy: "never" | "daily" | "hourly".
- Calibration CSV loader is unchanged.

## CLI (doser_cli)

- Argument parsing with `clap`. Commands: `Dose` and `SelfCheck`.
- Initializes logging early using `logging.file`, `logging.level`, and `logging.rotation`.
- Hardware path (when feature enabled) constructs `HardwareScale` and `HardwareMotor::try_new_with_en(step, dir, motor_en)`.
- Wires E‑stop if configured: active‑low, 5 ms poll by default.

### Logging

- `tracing` subscriber with `EnvFilter` level.
- Console: pretty or JSON.
- Optional file: non‑blocking appender; rotation per config.

## Error model

- `DoserError` variants surfaced from core; hardware errors are mapped appropriately. Scale timeouts are propagated distinctly.

## Testing

- Unit tests in `doser_core` (simulation) cover safety/filters/settle/E‑stop.
- Integration tests exercise CLI in simulation.
- CI runs fmt, clippy, and tests; hardware feature is compiled only on suitable platforms.

## Known limitations / next steps

- HX711 timing and motor pulse generation may need tuning on real hardware.
- E‑stop may require debounce. Add if spurious triggers appear.
- Optional ARM CI job could verify hardware feature builds on-device.

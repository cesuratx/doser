# Doser Architecture

This document describes the architecture of the Doser workspace, its crates, data flow, key Rust features used, and the safety invariants enforced by the core dosing logic.

## Workspace overview

The repository is a Rust workspace composed of multiple crates:

- doser_core: Hardware‑agnostic dosing logic (control loop, filtering, calibration, safety checks, state machine, builder).
- doser_traits: Thin abstraction layer defining hardware traits: Scale and Motor.
- doser_hardware: Hardware backends and a simulation backend. Includes an E‑stop checker factory for GPIO (feature‑gated) and simulated motor/scale.
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
      BUILDER -->|with_scale/with_motor| HWSEL{Hardware?
      (feature flag)}
      HWSEL -->|hardware| HW[HardwareScale & HardwareMotor]
      HWSEL -->|simulation| SIM[SimulatedScale & SimulatedMotor]
      BUILDER -->|with_filter/with_control/with_safety/with_timeouts/with_calibration| DOSER[Doser]
    end

    HW --> DOSER
    SIM --> DOSER

    subgraph Loop
      DOSER -->|step()| SCALE[Scale::read(timeout)]
      SCALE --> DOSER
      DOSER --> FILTER[Median + Moving Avg]
      FILTER --> SAFETY[Safety Guards
        (max_run, overshoot, watchdog, E‑stop)]
      SAFETY -->|abort/complete| STATUS{DosingStatus}
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

Important modules:

- lib.rs: Doser state machine, filtering, safety checks, builder API.
- error.rs: Workspace error types (e.g., `DoserError::{Config, Hardware, Timeout, State, Io}`).
- sampler.rs: Optional sampling helper (channel based), currently not used by the CLI but available for strategies.
- session.rs: Sample builder for sessions (range validation).
- tests/: Unit and integration tests for control, safety, calibration, error mapping, E‑stop, watchdog, and CLI integration.

### Control loop (step)

- Read scale with timeout -> raw counts.
- Convert to grams via calibration -> apply median prefilter and moving average.
- Update last weight; compute error to target.
- Safety checks in order:
  1. E‑stop callback.
  2. Max runtime (>= `max_run_ms`).
  3. Overshoot guard if `w > target + max_overshoot_g`.
  4. No‑progress watchdog (threshold epsilon for at least `no_progress_ms`).
- Completion: asymmetric; when `w >= target` stop motor, start settle timer, require stability for `stable_ms` before returning `Complete`.
- Otherwise choose coarse/fine speed by error magnitude and command motor (ensure start before set_speed).

### Filtering

- Median prefilter (`median_window`) to reject spikes.
- Moving average (`ma_window`) to reduce noise.
- Implemented with VecDeque buffers; median uses `total_cmp` for robust ordering.

### Time and determinism

- A `Clock` trait (`fn now_ms() -> u64`) is used instead of `Instant` for all time‑based logic (runtime cap, settle window, watchdog).
- `SystemClock` is the default. Tests can inject a `TestClock` via `DoserBuilder::with_clock(...)` and advance time deterministically.
- Uses `saturating_sub` for safe elapsed computations.

### Safety invariants

- Target grams must be within [0.1, 5000.0].
- Motor is stopped:
  - Immediately when E‑stop triggers.
  - Before starting settle on reaching/exceeding target.
  - On any abort (max runtime, overshoot, no progress) and on completion.
- Max runtime is a hard cap; checked using `>=` to allow deterministic tests at 0 ms.
- Overshoot guard prevents excessive overfill.
- No‑progress watchdog aborts if weight fails to increase by at least epsilon for `no_progress_ms` while running.
- Settling requires staying at/above target for at least `stable_ms` before reporting `Complete`.
- Motor `start()` is called before `set_speed()` the first time; speed updates may follow per step.
- Filter buffers never exceed configured window sizes.

## Traits (doser_traits)

- `trait Scale { fn read(&mut self, timeout: Duration) -> Result<i32, E>; }`
- `trait Motor { fn start(&mut self) -> Result<_, E>; fn set_speed(&mut self, s: u32) -> Result<_, E>; fn stop(&mut self) -> Result<_, E>; }`

These are implemented by simulation and (feature‑gated) hardware backends.

## Hardware (doser_hardware)

- Simulation: `SimulatedScale` and `SimulatedMotor` suitable for unit/integration tests.
- Hardware (skeletons): `HardwareScale` (e.g., HX711) and `HardwareMotor` (e.g., step/dir) for target platforms.
- E‑stop: `make_estop_checker(pin, active_low, poll_ms)` creates a closure polling a GPIO input; `Ok(checker)` is injected via `DoserBuilder::with_estop_check` when configured.
- Platform gating: hardware implementations behind `--features hardware` and target arch checks. The repo compiles on non‑ARM hosts by using simulations and stubs.

## Config (doser_config)

- Typed TOML config (pins, filter, control, timeouts, safety, logging). `serde` with sensible defaults.
- Calibration CSV loader (gain and offset, tare counts). CLI applies these via builder methods.

## CLI (doser_cli)

- Argument parsing with `clap`. Commands: `dose` and `self-check`.
- Loads TOML config first to initialize logging (including optional file sink).
- Maps typed config to core types, applies runtime safety overrides from CLI, injects calibration if provided.
- Selects hardware or simulation (feature‑gated) and wires E‑stop when configured.
- Runs the dosing loop until `Complete` or `Aborted`.

### Logging

- `tracing` subscriber with `EnvFilter` level.
- Console: pretty or JSON (via `--json`).
- Optional file: non‑blocking appender; writer guard kept alive via `OnceLock` to avoid log loss.
- Future work: rotation policies and ensuring log directories exist.

## Error model

- Core exposes `DoserError` (e.g., `Config`, `Hardware(String)`, `Timeout`, `State`, `Io`).
- Scale read timeout is currently mapped by a string heuristic; plan to replace with a typed hardware error enum for robust mapping.
- CLI uses `anyhow` for user‑facing errors and logs via `tracing`.

## Testing

- Unit tests cover: calibration, filtering, safety guards (runtime, overshoot, watchdog), settling behavior, E‑stop.
- Integration tests in `doser_core/tests/integration.rs` depend on the CLI binary being built (simulation mode) and validate end‑to‑end flow and logging initialization.
- Deterministic time: inject a test clock into `Doser` to advance time without sleeping.

## CI

- GitHub Actions workflow runs `cargo fmt --check`, `clippy`, unit/integration tests.
- Separate hardware‑feature compile check to ensure hardware code stays building under the feature flag.

## Extensibility

- Strategies: You can build more sophisticated dosing strategies by driving `Doser::step()` and composing with `Sampler`.
- Hardware: Implement platform‑specific Scale/Motor backends and enable `--features hardware` on target.
- UI: The `doser_ui` crate can integrate with GUIs or services, reusing `doser_core`.

## Known limitations / next steps

- Hardware drivers (HX711 read, motor pulse generation) are placeholders.
- Timeout error typing: Replace string heuristics with a concrete error enum propagated from hardware.
- Logging: optional rotation and ensuring the log directory exists/created early.
- Additional configuration validation and better defaults tuning for real hardware.

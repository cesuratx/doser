# Architecture Overview

Crates

- `doser_traits`: Interfaces (Scale, Motor, MonotonicClock)
- `doser_core`: Control loop, predictor, telemetry, runner
- `doser_config`: TOML schema, validation, calibration CSV + robust refit
- `doser_hardware`: Sim + hardware drivers, pacing, estop
- `doser_cli`: UX, logging, JSONL, RT setup

Data flow (high level)

1. CLI parses config → `doser_config::Config`
2. Build hardware (sim or real) implementing `doser_traits`
3. Run dosing via `doser_core::runner::run` or manual loop
4. Emit telemetry and JSONL; map domain errors to exit codes

Key decisions

- Traits decouple core from hardware; generics for tests, trait-objects for CLI.
- Fixed-point math inside control loop; predictor for early stop under latency.
- Minimal unsafe in RT helpers with explicit invariants.

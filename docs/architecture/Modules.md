# Modules & Files (map)

- `doser_traits/`
  - `clock.rs` → monotonic time abstraction
  - `lib.rs` → `Scale`, `Motor` traits
- `doser_config/`
  - `src/lib.rs` → `Config` types, validation, calibration loader, robust_refit
- `doser_core/`
  - `src/lib.rs` → `DoserCore`, control loop, filters, predictor, telemetry
  - `src/runner.rs` → high-level run orchestration and watchdogs
  - `src/sampler.rs` → background sampler thread
  - `src/error.rs` → domain errors (`AbortReason`)
- `doser_hardware/`
  - `src/lib.rs` → sim + hardware backends, pacing, estop utilities
- `doser_cli/`
  - `src/main.rs` → CLI, tracing, JSONL, RT helpers

Tests/Fuzz/Bench

- `*/tests/*.rs` integration/unit tests
- `fuzz/` libfuzzer target(s)
- `doser_core/benches` Criterion benches

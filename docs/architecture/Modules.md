# Modules & Files (map)

- `doser_traits/`
  - `clock.rs` → monotonic time abstraction
  - `lib.rs` → `Scale`, `Motor` traits
- `doser_config/`
  - `src/lib.rs` → `Config` types, validation, calibration loader, robust_refit
- `doser_core/`
  - `src/lib.rs` → crate root, re-exports
  - `src/core.rs` → `DoserCore`, control loop, predictor, safety, telemetry getters
  - `src/builder.rs` → type-state `DoserBuilder`
  - `src/calibration.rs`, `src/fixed_point.rs`, `src/conversions.rs` → counts↔cg math
  - `src/runner.rs` → high-level run orchestration and watchdogs (`run`, `run_observed`)
  - `src/sampler.rs` → background sampler thread
  - `src/error.rs` → domain errors (`AbortReason`)
- `doser_hardware/`
  - `src/lib.rs` → sim + hardware backends, pacing, estop utilities, `MAX_STEP_RATE_SPS`
  - `src/hx711.rs`, `src/util.rs` → HX711 driver and `Instant`-deadline bit-bang timing
- `doser_cli/`
  - `src/main.rs` → CLI entrypoint, command dispatch, JSONL result line, RT helpers
  - `src/cli.rs` → clap definitions; `src/dose.rs` → dose wiring and `--stats`
  - `src/monitor.rs` → live weight HTTP server; `src/jog.rs` → `motor` jog
  - `src/tracing_setup.rs` → tracing/log sinks (both console layers → stderr)

Tests/Fuzz/Bench/Examples

- `*/tests/*.rs` integration/unit tests (run in CI)
- `fuzz/` libfuzzer target for the config loader (**manual, not in CI**)
- `doser_core/benches/predictor.rs` Criterion bench (**manual, not in CI**)
- `doser_cli/examples/` runnable examples; `doser_hardware/examples/hx711_probe.rs` (Pi only)

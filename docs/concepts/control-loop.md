# Control Loop & State Machine

- Modes: fast approach → slow band → settle → done/abort.
- Early stop: `maybe_early_stop` forecasts final mass; stops motor preemptively.
- Safety: stall detection, max run, overshoot bounds, e-stop.

Pointers

- `doser_core/src/lib.rs` (DoserCore::step, maybe_early_stop, telemetry getters)
- `doser_core/src/runner.rs` (run\_\* variants and stall thresholds)

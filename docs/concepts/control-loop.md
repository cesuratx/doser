# Control Loop & State Machine

- Modes: fast approach → slow band → settle → done/abort.
- Early stop: `maybe_early_stop` forecasts final mass; stops motor preemptively.
- Safety: stall detection, max run, overshoot bounds, e-stop.

Pointers

- `doser_core/src/core.rs` (`DoserCore::step` / `step_from_raw`, `maybe_early_stop`,
  `poll_estop_stop`, telemetry getters)
- `doser_core/src/runner.rs` (`run` / `run_observed`, stall thresholds, `RunOutcome`)

Notes

- Stopping the motor clears the "started" flag, so if the weight later dips back below
  `target - epsilon` the loop restarts the motor to top the dose up; the no-progress deadline
  is re-armed at that restart.
- The CLI's `--stats` path calls the same `run_observed`, so watchdog behavior is identical
  with and without it.

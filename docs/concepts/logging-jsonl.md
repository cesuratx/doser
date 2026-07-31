# Logging & JSONL

- Tracing: initialized in `doser_cli/src/tracing_setup.rs`. **Both** console layers — pretty
  and JSON — write to **stderr**; stdout is reserved for the CLI's own output. An optional
  file sink (`[logging] file`, `rotation`) writes in parallel through a non-blocking appender
  whose `WorkerGuard` is held for the process lifetime.
- Level: `--log-level`, overridden by `RUST_LOG` when set. `[logging] level` in the TOML is
  parsed but ignored.
- JSONL: `--json` makes stdout emit one JSON object for the run, with stable keys:
  - timestamp, target_g, final_g, duration_ms, profile, slope_ema, stop_at_g, coast_comp_g, abort_reason
  - `slope_ema`/`stop_at_g`/`coast_comp_g` come from the runner's `RunOutcome` on every path,
    with or without `--stats`.
  - On an abort a second object follows with `reason`, `message`, and a `details` object for
    overshoot/max-runtime.
- Integration tests assert the schema; because logs are on stderr they cannot corrupt it.

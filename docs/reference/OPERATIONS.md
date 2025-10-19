# Operations Guide

This guide covers calibration file format, common runtime errors, and how to enable detailed control tracing and JSON logs for the CLI.

## Calibration CSV format

- File must be a CSV with the exact header:
  raw,grams
- Each row maps a raw sensor reading to grams.
- At least 2 rows; raw values must be strictly monotonic (no duplicates, no zig‑zag).
- An OLS fit computes grams = a\*raw + b across all rows; core uses `scale_factor=a` and `offset` as tare counts.

Example (3 rows):
raw,grams
100000,0.0
150000,5.0
200000,10.0

Usage:

- Pass the file to the CLI with `--calibration /path/to/calib.csv`.
- The header is strict; a bad header will be rejected with a clear error.

## Common errors and fixes

- HX711 timeout / Scale read timed out

  - What it means: No data-ready within the configured timeout.
  - Likely causes: Wrong DT/SCK pins, wiring/power issues, or timeout too low.
  - Fixes:
    - Verify 5V/GND and DT/SCK wiring.
    - Check the [pins] configuration values.
    - Increase `hardware.sensor_read_timeout_ms`.

- Missing motor or scale

  - What it means: The dosing engine was built without a motor and/or scale instance.
  - Likely causes: Hardware init failed or wasn’t wired into the builder.
  - Fixes:
    - Ensure hardware is created and passed via `with_motor(...)` / `with_scale(...)`.
    - In CLI use, verify config [pins] and permissions for GPIO (on hardware builds).

- Configuration validation errors
  - What it means: Required fields are absent or values are out of range.
  - Likely causes: Missing `[pins]` or invalid numeric ranges (e.g., `timeouts.sample_ms` >= 1, `control.hysteresis_g` >= 0).
  - Fixes:
    - Review your TOML and compare against the sample config.
    - Provide all required pins and sensible ranges.

## Control tracing and JSON logs

You can run the CLI with human-friendly or JSON logs and increase verbosity to trace control decisions.

- Human-friendly logs:

  - `doser --log-level debug dose --grams 5`

- JSON logs (newline-delimited JSON):

  - `doser --json --log-level trace dose --grams 5 > logs.jsonl`
  - Inspect with jq, for example:
    - `jq 'select(.level=="INFO")' logs.jsonl`
    - `jq -r '.timestamp + " " + .level + " " + (.fields.message // .message // "")' logs.jsonl'`

- Trace control
  - Use `--log-level trace` to enable detailed control-loop tracing.
  - Alternatively, set `RUST_LOG=trace`.

Tips:

- Combine `--json` with a file sink (see config) to keep terminal output clean.
- For the most detail, run with `--log-level trace` or `RUST_LOG=trace` and parse the JSON stream.

## Notes

- Errors are printed to stderr; stdout is reserved for normal output.
- The no‑progress watchdog aborts if weight doesn’t change by ≥ epsilon within the configured window.

## Real-time mode (rt)

- Linux: when `--rt` is enabled, the CLI attempts to set SCHED_FIFO priority, pin to CPU 0, and lock memory with `mlockall(MCL_CURRENT|MCL_FUTURE)`. This can reduce latency and jitter but may require elevated privileges and appropriate limits (e.g., `ulimit -l` for memlock, and allowing real-time scheduling). It can impact system responsiveness; prefer dedicated hosts.
- macOS: only `mlockall` is applied; real-time scheduling and CPU affinity are unavailable. Locking memory can increase pressure on the OS memory manager.
- Best-effort: if a step fails, a warning is printed and the run continues without that RT tweak.

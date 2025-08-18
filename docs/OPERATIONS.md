# Operations Guide

This guide covers calibration file format, common runtime errors, and how to enable detailed control tracing and JSON logs for the CLI.

## Calibration CSV format

- File must be a CSV with the exact header:
  raw,grams
- Each row maps a raw sensor reading to grams.
- Minimum of 2–3 rows recommended.

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
  - Likely causes: Missing `[pins]` or invalid numeric ranges (e.g., `control.epsilon_g` must be in [0.0, 1.0], `timeouts.sample_ms` >= 1).
  - Fixes:
    - Review your TOML and compare against the sample config.
    - Provide all required pins and sensible ranges.

## Control tracing and JSON logs

You can run the CLI with human-friendly or JSON logs and increase verbosity to trace control decisions.

- Human-friendly logs:

  - `doser_cli --log-level debug dose --grams 5`

- JSON logs (newline-delimited JSON):

  - `doser_cli --json --log-level trace dose --grams 5 > logs.jsonl`
  - Inspect with jq, for example:
    - `jq 'select(.level=="INFO")' logs.jsonl`
    - `jq -r '.timestamp + " " + .level + " " + (.fields.message // .message // "")' logs.jsonl'`

- Trace control
  - Use `--log-level trace` to enable detailed control-loop tracing.
  - Alternatively, set `RUST_LOG=trace`.

Tips:

- Combine `--json` with a file sink (see config) to keep terminal output clean.
- For the most detail, run with `--log-level trace` or `RUST_LOG=trace` and parse the JSON stream.

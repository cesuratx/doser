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

## The binary is `doser_cli`

The package `doser_cli` builds a binary named **`doser_cli`**, not `doser`. On a Pi that is
`target/release/doser_cli`; `install.sh` puts it at `/usr/local/bin/doser_cli`. (A release
workflow once shipped a nonexistent `doser` path — see the CHANGELOG.) Global flags
(`--config`, `--calibration`, `--json`, `--log-level`) go **before** the subcommand; the
subcommand's own flags (e.g. `--grams`) go after it.

```
doser_cli [--config FILE] [--calibration FILE] [--json] [--log-level LEVEL] <COMMAND>
COMMAND := dose | health | self-check | monitor | motor
```

## Subcommands at a glance

| Command | What it does | Prints on stdout |
| --- | --- | --- |
| `dose --grams N` | Runs the control loop to dispense N grams | `final: X.XX g`, or one JSONL object with `--json` |
| `health` | Reads the scale once, then starts/stops the motor | `✓ Scale: responsive (raw: …)`, `✓ Motor: responsive`, `Health check: OK` |
| `self-check` | Reads the scale for 1 s to estimate the HX711 rate. **Never touches the motor.** | `Detected HX711 rate: {10\|80} SPS` |
| `monitor` | Serves the live weight web UI | bind/status lines |
| `motor` | Jogs the motor at a fixed rate; no scale, no control loop | jog status lines |

Exit codes: `0` success, `1` generic error, `2` E-stop, `3` no-progress, `4` max-runtime,
`5` overshoot, `6` max-attempts.

## Control tracing and JSON logs

You can run the CLI with human-friendly or JSON logs and increase verbosity to trace control decisions.

- Human-friendly logs:

  - `doser_cli --log-level debug dose --grams 5`

- JSON logs (newline-delimited JSON):

  - Log records go to **stderr**, so redirect stderr, not stdout:
    `doser_cli --json --log-level trace dose --grams 5 2> logs.jsonl`
  - stdout carries the single JSONL *result* line, so
    `doser_cli --json dose --grams 5 > result.jsonl` gives exactly one object with the keys
    `timestamp,target_g,final_g,duration_ms,profile,slope_ema,stop_at_g,coast_comp_g,abort_reason`.
    On an abort, a second object describing the error is printed with `reason`, `message`
    and (for overshoot/max-runtime) a `details` object.
  - Inspect the log stream with jq, for example:
    - `jq 'select(.level=="INFO")' logs.jsonl`
    - `jq -r '.timestamp + " " + .level + " " + (.fields.message // .message // "")' logs.jsonl`

- Trace control
  - Use `--log-level trace` to enable detailed control-loop tracing.
  - Alternatively, set `RUST_LOG=trace`. **`RUST_LOG` wins over `--log-level`** when set.

Tips:

- Combine `--json` with a file sink (`[logging] file`) to keep the terminal clean.
- For the most detail, run with `--log-level trace` or `RUST_LOG=trace` and parse the JSON stream.
- `dose --stats` prints loop-latency statistics (samples, period, min/avg/max/stdev latency,
  missed deadlines) to stderr. It runs the same control loop as a plain `dose`, so all safety
  watchdogs apply.

## Live weight monitor

```bash
doser_cli --config /etc/doser_config.toml monitor --bind 127.0.0.1 --port 8080
```

- Serves a self-contained page (no CDN) that polls `GET /reading` about ten times a second
  and can tare via `POST /tare` / `POST /tare/clear`.
- `--bind` defaults to `0.0.0.0`. **The UI has no authentication and no TLS.** Binding
  anything but loopback publishes the live scale feed — and the tare buttons — to everyone
  who can route to the machine. The CLI prints a `WARNING:` line on stderr in that case.
  Prefer `--bind 127.0.0.1` plus an SSH tunnel: `ssh -L 8080:127.0.0.1:8080 pi@doser.local`.
- `--hz` overrides the sampling rate (default: `filter.sample_rate_hz`).
- The two POST endpoints require the header **`X-Doser-Monitor`**; without it they answer
  `403 {"ok":false,"error":"missing X-Doser-Monitor header"}`. The header makes cross-origin
  POSTs non-"simple" so browsers must preflight them, and the server sends no CORS headers,
  which is what prevents a hostile page from taring the scale mid-dose. Scripted use:

  ```bash
  curl -X POST -H 'X-Doser-Monitor: 1' http://127.0.0.1:8080/tare
  curl -X POST -H 'X-Doser-Monitor: 1' http://127.0.0.1:8080/tare/clear
  ```

- A tare requested before the first successful sample answers `409` with
  `no reading yet — check the wiring`.
- The POSTs also require the request's `Host` to look like a LAN address (IP literal,
  single-label name, or a `.local` / `.lan` / `.internal` / `.home.arpa` suffix). Reaching the
  Pi through a public DNS name that resolves to a private IP (e.g. Tailscale MagicDNS) will
  refuse tare with `Host is not a LAN address`; use the IP form. `GET /` and `GET /reading`
  are not gated.

## Motor jog

```bash
doser_cli --config /etc/doser_config.toml motor --sps 400 --ms 2000 --dir cw
doser_cli --config /etc/doser_config.toml motor --sps 400 --steps 800 --dir ccw
```

- No scale and no control loop — this is a bring-up/bench tool for checking STEP/DIR wiring,
  direction and the driver's current limit.
- `--sps` is validated by clap against `1..=5000` (`doser_hardware::MAX_STEP_RATE_SPS`, the
  clamp both motor backends apply). Out-of-range exits 2 with
  `invalid value '20000' for '--sps <HZ>': 20000 is not in 1..=5000`.
- `--ms` (default 1000) sets the duration; `--steps` overrides it and is approximate — it
  becomes `ceil(steps / sps)` seconds of paced stepping.
- Ctrl-C stops the motor promptly; the driver is de-energized when the motor is dropped.

## Notes

- All log records — pretty and JSON — go to stderr; stdout is reserved for the CLI's own
  output (result lines and status lines).
- The no‑progress watchdog aborts if weight doesn’t change by ≥ epsilon within the configured
  window **while the motor is commanded to run**; time spent deliberately stopped (settling,
  or a predictor early stop) does not count against it.
- If the weight falls back below `target - epsilon` after the motor has been stopped, the
  motor is restarted to top the dose up, and the no-progress deadline is re-armed at that
  moment.

## Real-time mode (rt)

- Linux: when `--rt` is enabled, the CLI attempts to set SCHED_FIFO priority, pin to CPU 0, and lock memory with `mlockall(MCL_CURRENT|MCL_FUTURE)`. This can reduce latency and jitter but may require elevated privileges and appropriate limits (e.g., `ulimit -l` for memlock, and allowing real-time scheduling). It can impact system responsiveness; prefer dedicated hosts.
- macOS: only `mlockall` is applied; real-time scheduling and CPU affinity are unavailable. Locking memory can increase pressure on the OS memory manager.
- Best-effort: if a step fails, a warning is printed and the run continues without that RT tweak.

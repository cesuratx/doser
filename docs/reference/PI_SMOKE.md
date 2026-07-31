# Raspberry Pi Smoke Test

Quick checks to verify wiring and configuration on a Raspberry Pi with the hardware feature.
Run them in order; each one narrows the fault to a smaller part of the machine.

## Build

```bash
cargo build -p doser_cli --features hardware --release
```

The binary is `target/release/doser_cli` — **not** `doser`. It needs GPIO access: be in the
`gpio` group (`/dev/gpiomem*`); no root required.

## 1. Scale + motor reachable — `health`

```bash
./target/release/doser_cli --config ./etc/doser_config.toml health
```

- Expects on stdout:

  ```
  ✓ Scale: responsive (raw: 123456)
  ✓ Motor: responsive

  Health check: OK
  ```

- This is the only check that touches the motor: it sets 100 sps, starts, waits 50 ms, stops.
- Exit code is non-zero and the failing half is reported as `✗ Scale: …` / `✗ Motor: …`.
- If the scale times out, check DT/SCK pins and power, and raise
  `hardware.sensor_read_timeout_ms`.
- The printed `raw` count is what you record for calibration points.

## 2. Sample rate — `self-check`

```bash
./target/release/doser_cli --config ./etc/doser_config.toml self-check
```

- Reads the scale for one second and prints `Detected HX711 rate: 10 SPS` (or `80 SPS`).
  It does **not** touch the motor and does **not** print `OK`.
- Treat the answer as a hint: the detector labels any inter-read gap under 50 ms as 80 SPS,
  so a desynced free-running read is mislabeled as healthy. The authoritative check is the
  probe (see below) — HARDWARE_LESSONS Lesson 4.
- `filter.sample_rate_hz` and the per-read timeout (`timeouts.sample_ms` / `sensor_ms`) must
  match the real rate: at 10 SPS a sample arrives every ~90–100 ms, so a 50 ms timeout times
  out on every read.

## 3. Authoritative HX711 probe

```bash
cargo run -p doser_hardware --example hx711_probe --features hardware
HX711_STREAM=1 cargo run -p doser_hardware --example hx711_probe --features hardware
```

Environment: `HX711_DT`, `HX711_SCK`, `HX711_HIGH_US`, `HX711_READS`, `HX711_STREAM`.
A healthy unloaded bridge sits mid-range and swings by hundreds of thousands of counts under
a hard press. A hard rail (`±8388608`) or a value that hops between rails across runs is an
analog-side fault — see `docs/ops/HARDWARE_LESSONS.md` before touching code.

Note: `pinctrl set` pulses are millisecond-wide and trip the HX711's power-down, so `pinctrl`
cannot validate fast bit-bang timing. `pinctrl get 5,6` for pin states is still useful.

## 4. Motor jog (no scale, no control loop)

```bash
./target/release/doser_cli --config ./etc/doser_config.toml motor --sps 400 --ms 2000
./target/release/doser_cli --config ./etc/doser_config.toml motor --sps 400 --steps 800 --dir ccw
```

Confirms STEP/DIR wiring, direction, and the A4988/DRV8825 current limit without any
dependence on the scale. `--sps` is limited to `1..=5000`; `--steps` is approximate
(duration-derived). Ctrl-C stops the motor promptly.

## 5. Live weight view (optional)

```bash
./target/release/doser_cli --config ./etc/doser_config.toml monitor --bind 127.0.0.1
# from your laptop: ssh -L 8080:127.0.0.1:8080 pi@doser.local  → http://localhost:8080
```

The UI is unauthenticated and unencrypted; `--bind` defaults to `0.0.0.0` and the CLI prints
a `WARNING:` on stderr when it is not loopback. `POST /tare` and `POST /tare/clear` require
the `X-Doser-Monitor` header (`curl -X POST -H 'X-Doser-Monitor: 1' …`).

## 6. Dose (small)

```bash
./target/release/doser_cli --config ./etc/doser_config.toml dose --grams 1
```

- Watch logs on **stderr**. stdout gets only `final: X.XX g` (or the JSONL line with `--json`).
- Use `--log-level debug` (before the subcommand) or `--json` for structured logs.
- `--stats` prints loop-latency statistics to stderr; the safety watchdogs apply on that path
  exactly as they do without it.

## 7. E-Stop

- If `pins.estop_in` is wired, press to ensure the run aborts immediately.
- Debounce (`estop.debounce_n`) is applied; once tripped, the latch holds until `begin()`
  (the next run).

## Logs

- Console level via `--log-level` or `RUST_LOG` (`RUST_LOG` wins when set).
- Optional file sink via `[logging] file` and `rotation` in the config. `[logging] level` is
  parsed but ignored.

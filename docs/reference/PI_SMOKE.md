# Raspberry Pi Smoke Test

Quick checks to verify wiring and configuration on a Raspberry Pi with the hardware feature.

## Build

```bash
cargo build -p doser_cli --features hardware --release
```

## Self-Check

```bash
./target/release/doser_cli --config ./etc/doser_config.toml self-check
```

- Expects: scale read ok, motor start/stop ok, "OK" on stdout.
- If scale times out, check pins and power; increase `hardware.sensor_read_timeout_ms`.

## Dose (small)

```bash
./target/release/doser_cli --config ./etc/doser_config.toml dose --grams 1
```

- Watch logs. Use `--log-level debug` or `--json` for structured logs.

## E-Stop

- If `pins.estop_in` is wired, press to ensure the run aborts immediately.
- Debounce is applied; once tripped, latch holds until `begin()` (next run).

## Logs

- Console level via `--log-level` or `RUST_LOG`.
- Optional file sink via `[logging] file` and `rotation` in the config.

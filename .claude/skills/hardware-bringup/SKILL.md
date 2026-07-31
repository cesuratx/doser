---
name: hardware-bringup
description: Test and diagnose the doser's HX711 load cell and motor on the Raspberry Pi. Use when asked to test the scale/HX711/load cell, bring up hardware, debug a sensor reading, or verify GPIO wiring on the Pi. Codifies the proven bring-up procedure so sessions don't re-derive it.
---

# Hardware bring-up & diagnosis (doser, Raspberry Pi)

Goal: determine whether the HX711/load-cell/motor work, and if not, isolate the fault to a
specific layer (power → digital link → timing → analog/load-cell → config). Always diagnose
at signal level and prefer evidence (probe output, measured timing) over assertion.

**Before starting:** recall Claude memory (`hardware-bringup-procedure`, `pi5-gpio-timing-gotcha`,
`hx711-current-state`) and skim `docs/ops/HARDWARE_LESSONS.md`. **After notable findings:**
append a dated entry to that log and update memory.

## 0. Preconditions
- Pi, in `gpio` group, `/dev/gpiomem*` present → no root needed. `uname -m` = aarch64.
- Pins (BCM): HX711 DT=5, SCK=6; motor STEP=23, DIR=24. Confirm against `etc/doser_config.toml`.

## 1. Build the real backend
```bash
cargo build --release -p doser_cli --features hardware   # binary: target/release/doser_cli
```

## 2. Smoke test via CLI
```bash
./target/release/doser_cli --config etc/doser_config.toml health      # one raw read + motor pulse
./target/release/doser_cli --config etc/doser_config.toml self-check  # sample-rate detector
```
`health` is the one that gives you a number: it prints `✓ Scale: responsive (raw: <count>)` on
stdout, so `health 2>/dev/null | grep raw` isolates it. Do **not** pipe `self-check` looking for
a raw count — it only ever prints `Detected HX711 rate: N SPS`, and all tracing (any
`--log-level`) goes to stderr, so a `| grep raw` there matches nothing regardless of verbosity.

Caveat: `self-check`'s "80 SPS" detector is unreliable (labels any <50 ms gap as 80 SPS).
Trust the probe, not this.

## 2b. Isolate the two halves
Split motor from scale before you debug either — most bring-up faults are in one half only.

```bash
# Motor only: no scale read, no control loop. Listen/watch for stepping.
./target/release/doser_cli --config etc/doser_config.toml motor --sps 400 --ms 2000
./target/release/doser_cli --config etc/doser_config.toml motor --sps 400 --steps 800 --dir ccw
```
`--sps` is 1..=5000 (the driver clamp); `--steps` is *approximate* — it is converted to a
duration of N/sps seconds, so jitter lands a step or two either side. Ctrl-C stops early.

```bash
# Scale only: live raw counts in a browser, with tare buttons.
./target/release/doser_cli --config etc/doser_config.toml monitor --bind 127.0.0.1
```
This is the fastest way to watch counts move while you press the cell — better than re-running
the probe. `--bind` defaults to `0.0.0.0`, which exposes an **unauthenticated** feed to the whole
LAN; use `127.0.0.1` and an SSH tunnel unless you trust the network. `--port` defaults to 8080,
`--hz` overrides `filter.sample_rate_hz`.

The `POST /tare` and `POST /tare/clear` endpoints require an `X-Doser-Monitor: 1` header (a CSRF
defence — without it a random page in the operator's browser could zero the scale mid-dose), so a
hand-rolled curl needs it:
```bash
curl -s http://127.0.0.1:8080/reading                                  # JSON: raw, grams, sps, tare state
curl -s -X POST -H 'X-Doser-Monitor: 1' http://127.0.0.1:8080/tare     # {"ok":true}; 403 without the header
curl -s -X POST -H 'X-Doser-Monitor: 1' http://127.0.0.1:8080/tare/clear
```

## 3. Authoritative probe (Instant-timed bit-bang)
```bash
cargo run -p doser_hardware --example hx711_probe --features hardware
# live load test (press the cell while it runs):
HX711_STREAM=1 HX711_READS=200 cargo run -p doser_hardware --example hx711_probe --features hardware
```
Env knobs: `HX711_DT`, `HX711_SCK`, `HX711_HIGH_US`, `HX711_LOW_US`, `HX711_READS`, `HX711_STREAM`.
The probe prints raw value, the 24 bits, data-ready wait, and whether DT returns high after
clocking (proves shifting works).

## 4. Pin-level probing (when the digital link is suspect)
```bash
pinctrl get 5,6                 # line state
pinctrl set 5 ip pd && pinctrl get 5   # driven (stays hi) vs floating (follows pull)
```
`pinctrl` pulses are millisecond-wide → they trip the HX711 >60µs power-down, so they CANNOT
validate fast clocking. Use the Rust probe for timing.

## 5. Interpreting results
| Symptom | Meaning | Action |
| --- | --- | --- |
| Stable `0x800000` / `0x7FFFFF`, no response to load | Railed analog input | Check load-cell wiring E+/E−/A+/A−, channel, dead cell |
| `0` or `-1` that varies between runs | Digital/timing fault | Use Instant-based pulse delays (see Lesson 1) |
| DT never returns high after 25 pulses | Pulses not reaching chip | Check SCK wiring/timing |
| Inter-sample gap ~90 ms | Running at 10 SPS (RATE pin low) | Set `sample_rate_hz=10`, `sensor_ms`≥120; or bridge RATE high for 80 SPS |
| Value swings under finger pressure | Load cell healthy | Proceed to calibration |

## 6. Record findings
Append a dated entry to `docs/ops/HARDWARE_LESSONS.md` and write a memory note. That is the
point of this skill — never re-derive a solved fault.

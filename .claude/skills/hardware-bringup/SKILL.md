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
./target/release/doser_cli --config etc/doser_config.toml --log-level trace self-check | grep raw
```
Caveat: `self-check`'s "80 SPS" detector is unreliable (labels any <50 ms gap as 80 SPS).
Trust the probe, not this.

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

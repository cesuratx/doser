# Hardware Lessons Log

A running, append-only log of hardware/electronics lessons learned during bring-up and
operation. **Purpose:** so no future session (human or AI) re-derives a problem we have
already solved. Add a dated entry whenever you burn time on a hardware fault, a timing
quirk, or a non-obvious electrical behavior. Newest first.

Companion docs: [HARDWARE_SETUP.md](../guides/HARDWARE_SETUP.md) (wiring/BOM),
[Runbook.md](./Runbook.md) (operations), [ADRs](../adr/) (design decisions).

---

## 2026-06-21 — HX711 bring-up on Raspberry Pi 5

**Platform:** Pi 5 (aarch64, RP1 GPIO), HX711 on DT=GPIO5 / SCK=GPIO6. User in `gpio`
group, `/dev/gpiomem*` present → no root needed.

### Lesson 1 — Bit-bang pulse timing: use an `Instant` deadline, not a spin count
The driver's `busy_wait_min_1us` calibrated a `std::hint::spin_loop()` iteration count to
approximate 1µs. On the RP1 GPIO this produced sub-microsecond, jittery SCK pulses that the
HX711 intermittently missed: the real driver returned **unstable garbage** (`raw: 0`, then
`raw: -1` on the next run) while a properly-timed standalone probe read a rock-stable value.
`spin_loop()` is ~1 cycle on aarch64, so the calibration under-counts.

**Fix:** spin against a monotonic `Instant` deadline (`while start.elapsed() < ~1.5µs`).
After the fix, `doser_cli health` reads matched the probe exactly on every run. Apply the
same Instant-based pattern to any future bit-banged signal (motor STEP edges, new sensors).
File: `doser_hardware/src/util.rs`.

### Lesson 2 — A hard rail (`0x800000` / `0x7FFFFF`) means a railed analog input
The load cell read **exactly `-8388608` (0x800000, negative full-scale), unchanging across
200 reads / 20 s, with zero response to physically pressing the cell.** A correctly wired
load cell sits mid-range and swings strongly under load. A hard rail = the bridge isn't
delivering a usable differential signal → **load-cell wiring fault** (E+/E−/A+/A− swapped,
loose, on the wrong channel, or a dead cell), not software. Diagnose the analog side before
touching code.

### Lesson 3 — Confirm the actual sample rate; this board is 10 SPS, not 80
Measured inter-sample gap ~90 ms (~11 Hz) = the HX711's **10 SPS** mode (RATE pin tied LOW
on the breakout). Config assumed 80 SPS. At 10 SPS a sample needs ~100 ms, so the per-read
timeout must exceed that — `sensor_ms` was 50 (would time out every read). Updated
`etc/doser_config.toml` to `sample_rate_hz = 10`, `sensor_ms = 200`. For 80 SPS, bridge the
RATE pad HIGH on the board.

### Lesson 4 — Don't trust `self-check`'s SPS detector; and `pinctrl` can't validate fast clocking
`self-check` labels any inter-read gap <50 ms as "80 SPS", so a desynced free-running read
(microsecond gaps) is mislabeled as healthy 80 SPS — a false positive. Verify with the probe
instead. Also, `pinctrl set` pulses are millisecond-wide and trip the HX711's >60µs
power-down (which also drives DOUT high), so `pinctrl` cannot validate fast bit-bang timing —
use the Rust probe (`cargo run -p doser_hardware --example hx711_probe --features hardware`).

**Diagnostic toolkit added:** `doser_hardware/examples/hx711_probe.rs` (Instant-timed probe;
`HX711_STREAM=1` for the live press test). Full procedure lives in Claude memory
(`hardware-bringup-procedure`).

# Hardware Lessons Log

A running, append-only log of hardware/electronics lessons learned during bring-up and
operation. **Purpose:** so no future session (human or AI) re-derives a problem we have
already solved. Add a dated entry whenever you burn time on a hardware fault, a timing
quirk, or a non-obvious electrical behavior. Newest first.

Companion docs: [HARDWARE_SETUP.md](../guides/HARDWARE_SETUP.md) (wiring/BOM),
[Runbook.md](./Runbook.md) (operations), [ADRs](../adr/) (design decisions).

---

## 2026-06-21 — CORRECTION: cell was NOT dead — the "0.2 Ω short" was a solder bridge on E+/E−

**This overturns the entry below ("bridge confirmed dead").** After re-soldering the E−/E+
joints, the probe reads a **stable, balanced, non-railed value (~−74,500, span ~200 counts over
200 reads)** with a clean data-ready handshake at 10 SPS. That is exactly what a powered,
roughly-balanced Wheatstone bridge looks like at no load — and it is **impossible** for a cell
whose excitation pair is shorted (0.2 Ω) and whose signal pair is open. Therefore:

- The earlier **E−↔E+ = 0.2 Ω** was a **solder bridge across the E+/E− pads/joint**, not an
  internal short — separating the joints cleared it and de-railed the input.
- The earlier **A+/A− = OL** was very likely the **enameled-wire false-open** the dead-cell
  entry itself warned about (insulated ends read OL until scraped), not a broken signal pair.
  A floating A+/A− would re-rail the input; it is stable, so A+/A− are connected.

**Lesson:** a single 0.2 Ω pair + open pairs is *also* the signature of a **board-side solder
bridge plus un-scraped probe contacts** — do **not** condemn the cell on ohm-out alone.
Re-flow/re-solder the suspect joints and re-probe *before* declaring a cell dead. Confirm the
bridge electrically (stable, non-railed read) — that is the authoritative test, not the meter.

**RESOLVED — cell confirmed fully healthy.** The flat (~200-count) readings were **not loaded**,
not a fault: the cell sat flat on the bench, so the beam couldn't bend and the gauges saw no
strain. The first hand-press runs were also too gentle / didn't actually flex the beam. A
**short, hard** press (`HX711_READS=50`, push as hard as possible) produced a clean monotonic
swing **−72,000 → +745,000 (span ~817,000 counts)** vs ~200 unloaded. Corroborating clue seen
before this: the resting zero drifted ~1,400 counts *between* runs from handling — a dead cell
wouldn't drift, so the gauges were always alive; they just weren't being loaded.

**Lesson:** before suspecting a de-railed but flat load cell, make sure it is actually being
**flexed** — a bar/beam cell only outputs when mounted as a cantilever (fixed end rigid, load
on the free end). Lying flat + finger pressure ≈ no strain. Use a short hard press or a clamped
known weight to confirm; a healthy low-capacity bar swings by hundreds of thousands of counts.
Full chain now verified: power → digital link (DT handshake, 10 SPS) → balanced bridge → load
response. **Next: scale calibration** (raw→grams with known reference weights).

## 2026-06-21 — Load-cell bridge confirmed dead (open signal pair + shorted excitation)

**Resolves the Lesson 2/5 blocker.** Followed Lesson 5's next step and ohmed all six pairs of
the four load-cell wires (E+/E−/A+/A−). A healthy Wheatstone bridge reads a **finite**
resistance on *every* pair: ~1000 Ω across E+/E− (input) and A+/A− (output), and ~hundreds of
Ω to ~1 kΩ on each E↔A arm (≈405 Ω for a 350 Ω cell). Measured instead:

- **E− ↔ E+ = 0.2 Ω (continuity beep)** — a dead short, not a bridge (0.2 Ω ≈ meter-lead R).
- **All five other pairs = OL (open).** A+ and A− are disconnected from everything.

That is no resistor network at all: the excitation lines are shorted together and the signal
pair is fully open → a **physically destroyed/broken bridge**, i.e. a **dead load cell**, not
a Pi-side wiring, HX711, GPIO, or software fault. This is the root cause of the wandering rail
(Lessons 2 & 5): a floating A+/A− differential input. **Fix = replace the load cell.**

Diagnostic rule for next time: a 4-wire load cell is a bridge — *every* wire pair must read
finite. **One pair near 0 Ω + any pair open ⇒ the cell is dead; stop debugging the Pi side.**
Caveats before condemning: probe on bare metal (enameled/insulated ends read OL until
scraped), and confirm color→pad mapping — though no remapping turns a 0.2 Ω short + two open
wires into a valid bridge.

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

### Lesson 5 — A rail that *wanders between both extremes* across runs = a floating differential input
Follow-up to Lesson 2. After re-soldering the **DAT** (digital DOUT) joint, the reading was
`0` on one probe run and a hard `+8388607` (0x7FFFFF, positive full-scale) on the next — vs.
`-8388608` (negative full-scale) in the prior session. Still **zero response to pressing the
cell.** A value that hops between +FS / −FS / 0 from run to run (rather than sitting at one
stable rail) is the signature of a **floating differential analog input** — an open in the
load-cell **signal pair (A+/A−)**: with nothing driving the differential input, the ADC
integrator drifts to whichever rail it last leaned toward.

Crucially, this **isolated the fault away from DAT**: the digital link reads cleanly and
consistently (chip drives DT high at idle — confirmed it overrides a GPIO pull-down via
`pinctrl set 5 ip pd`), so the earlier unstable values were *not* a bad DAT joint. **Stop
re-soldering DAT.** Continuity-check the four load-cell wires into the HX711 (E+, E−, A+, A−);
prioritize the A+/A− pair (cold/open joint or swap). A DAT–SCK solder bridge was also ruled
out: a bridge would read all **1s** (DT follows SCK high during sampling), but the probe read
all **0s**.

**Diagnostic toolkit added:** `doser_hardware/examples/hx711_probe.rs` (Instant-timed probe;
`HX711_STREAM=1` for the live press test). Full procedure lives in Claude memory
(`hardware-bringup-procedure`).

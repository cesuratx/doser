# Hardware Setup Guide

**For**: Doser — Coffee Bean Dosing System  
**Platform**: Raspberry Pi (3B+, 4B, or Zero 2 W)  
**Difficulty**: Beginner-friendly

---

## Bill of Materials (BOM)

| #   | Component               | Model / Spec                           | Qty | Est. Cost |
| --- | ----------------------- | -------------------------------------- | --- | --------- |
| 1   | **Raspberry Pi**        | Pi 4B (2 GB+) or Pi Zero 2 W           | 1   | $15–45    |
| 2   | **Load Cell**           | 5 kg strain gauge (TAL220B or similar) | 1   | $8        |
| 3   | **HX711 Amplifier**     | HX711 breakout board (24-bit ADC)      | 1   | $3        |
| 4   | **Stepper Motor**       | NEMA 17, 1.8°/step, 40–60 Ncm, 12 V    | 1   | $12       |
| 5   | **Motor Driver**        | A4988 or DRV8825 breakout              | 1   | $3        |
| 6   | **Power Supply**        | 12 V 2 A DC for the motor              | 1   | $8        |
| 7   | **5 V supply for Pi**   | USB-C 5 V 3 A (official or quality)    | 1   | $10       |
| 8   | **E-stop button**       | Red mushroom, normally-open            | 1   | $4        |
| 9   | **Dupont jumper wires** | F-F and M-F, 20 cm, assorted           | 20+ | $3        |
| 10  | **Breadboard or PCB**   | Half-size or full-size                 | 1   | $3        |
| 11  | **100 µF capacitor**    | Electrolytic, ≥16 V                    | 1   | $0.20     |
| 12  | **Misc**                | screws, standoffs, zip ties            | —   | $5        |
|     | **Total**               |                                        |     | **~$115** |

> **Tip**: Order a spare HX711 — they are cheap and occasionally defective.

---

## 1. Raspberry Pi Preparation

### 1.1 Flash the OS

1. Download **Raspberry Pi OS Lite (64-bit)** from https://www.raspberrypi.com/software/.
2. Flash with **Raspberry Pi Imager** onto a 16 GB+ micro-SD card.
3. In Imager settings, enable **SSH**, set your Wi-Fi credentials, and set a hostname (e.g. `doser`).

### 1.2 First Boot

```bash
ssh pi@doser.local          # default password: raspberry — change it!
sudo apt update && sudo apt upgrade -y
sudo apt install -y build-essential git
```

### 1.3 Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup show                   # confirms toolchain from rust-toolchain.toml
```

### 1.4 Clone and Build

```bash
git clone <your-repo-url> ~/doser
cd ~/doser
cargo build -p doser_cli --features hardware --release
```

> The `hardware` feature flag enables the real GPIO/HX711 driver.  
> Without it, the build uses simulated backends for testing on non-Pi machines.

---

## 2. Pin Assignments (BCM Numbering)

The software uses **BCM GPIO numbers**, not physical pin numbers.

| Signal                     | BCM GPIO | Physical Pin                 | Direction | Notes                                    |
| -------------------------- | -------- | ---------------------------- | --------- | ---------------------------------------- |
| **HX711 DT (Data)**        | 5        | 29                           | IN        | Goes LOW when data ready                 |
| **HX711 SCK (Clock)**      | 6        | 31                           | OUT       | Idle LOW; pulse HIGH to clock bits       |
| **Motor STEP**             | 23       | 16                           | OUT       | Pulse to advance one step                |
| **Motor DIR**              | 24       | 18                           | OUT       | HIGH = CW, LOW = CCW (depends on wiring) |
| **Motor EN** _(optional)_  | 25       | 22                           | OUT       | Active-LOW enable; pull LOW to run       |
| **E-stop IN** _(optional)_ | 12       | 32                           | IN        | Normally HIGH; pressed = LOW             |
| **GND**                    | —        | 6, 9, 14, 20, 25, 30, 34, 39 | —         | Use any GND pin                          |
| **3.3 V**                  | —        | 1, 17                        | —         | Power for HX711 VCC                      |
| **5 V**                    | —        | 2, 4                         | —         | **DO NOT** connect to GPIO inputs        |

These defaults match `doser_config.toml`:

```toml
[pins]
hx711_dt  = 5
hx711_sck = 6
motor_step = 23
motor_dir  = 24
# motor_en  = 25       # uncomment if wired
# estop_in  = 12       # uncomment if wired
```

> You can change pins — just update the TOML to match your wiring.

### Pin Map (Raspberry Pi 40-pin header)

```
                     +-----+-----+
              3.3V   |  1  |  2  |  5V
   (SDA1) GPIO  2   |  3  |  4  |  5V
   (SCL1) GPIO  3   |  5  |  6  |  GND
          GPIO  4   |  7  |  8  |  GPIO 14 (TXD)
              GND   |  9  | 10  |  GPIO 15 (RXD)
          GPIO 17   | 11  | 12  |  GPIO 18
          GPIO 27   | 13  | 14  |  GND
          GPIO 22   | 15  | 16  |  GPIO 23  ← MOTOR STEP
              3.3V  | 17  | 18  |  GPIO 24  ← MOTOR DIR
  (MOSI) GPIO 10   | 19  | 20  |  GND
  (MISO) GPIO  9   | 21  | 22  |  GPIO 25  ← MOTOR EN (opt)
  (SCLK) GPIO 11   | 23  | 24  |  GPIO  8
              GND   | 25  | 26  |  GPIO  7
          GPIO  0   | 27  | 28  |  GPIO  1
  HX711 DT → GPIO  5   | 29  | 30  |  GND
  HX711 SCK → GPIO  6   | 31  | 32  |  GPIO 12  ← E-STOP (opt)
          GPIO 13   | 33  | 34  |  GND
          GPIO 19   | 35  | 36  |  GPIO 16
          GPIO 26   | 37  | 38  |  GPIO 20
              GND   | 39  | 40  |  GPIO 21
                     +-----+-----+
```

---

## 3. Wiring — Step by Step

### 3.1 HX711 Load Cell Amplifier

The HX711 converts the analog load cell signal into a 24-bit digital reading.

```
Load Cell (4-wire)         HX711 Board             Raspberry Pi
─────────────────          ───────────             ──────────────
RED    (E+)  ───────────→  E+
BLACK  (E-)  ───────────→  E-
WHITE  (A-)  ───────────→  A-
GREEN  (A+)  ───────────→  A+
                           VCC  ←────────────────  Pin 1  (3.3 V)
                           GND  ←────────────────  Pin 6  (GND)
                           DT   ────────────────→  Pin 29 (GPIO 5)
                           SCK  ←────────────────  Pin 31 (GPIO 6)
```

**Important notes:**

- Power the HX711 from **3.3 V** (not 5 V) when connecting DT directly to Pi GPIO.
- Keep HX711 wires **short** (< 30 cm) and away from the motor/power wires.
- If readings are noisy, use **shielded cable** for the load cell wires.
- The load cell colors above are the most common convention; **verify with your cell's datasheet**.

### 3.2 Stepper Motor + A4988 Driver

The A4988 driver controls the stepper by receiving STEP and DIR pulses from the Pi.

```
Raspberry Pi          A4988 Driver            Motor + Power
──────────────        ────────────            ─────────────
Pin 16 (GPIO 23) ──→ STEP
Pin 18 (GPIO 24) ──→ DIR
Pin 22 (GPIO 25) ──→ ENABLE  (optional)
                      VDD  ←──────────────── 3.3 V (logic supply)
                      GND  ←──────────────── Pi GND
                      VMOT ←──────────────── 12 V PSU (+)
                      GND  ←──────────────── 12 V PSU (−)
                      1A ──────────────────→ Motor coil A+
                      1B ──────────────────→ Motor coil A−
                      2A ──────────────────→ Motor coil B+
                      2B ──────────────────→ Motor coil B−
```

#### Critical: Motor Power

1. **Place a 100 µF capacitor** across VMOT and GND on the A4988 to absorb voltage spikes.
2. **Never connect or disconnect the motor while powered** — back-EMF can destroy the driver.
3. Connect the 12 V supply **GND** to the **same GND rail** as the Pi (common ground).

#### A4988 Current Limit

Before powering on, **set the current limit** on the A4988 to match your motor:

1. Find your motor's rated current (e.g. 1.5 A for a typical NEMA 17).
2. Measure the VREF test point on the A4988 with a multimeter.
3. Adjust the potentiometer: `VREF = Imax × 8 × Rsense` (for 0.1 Ω sense resistors: `VREF = Imax × 0.8`).
4. Example: 1.5 A motor → VREF ≈ 1.2 V.

#### Microstepping (Optional)

The A4988 supports up to 1/16 microstepping via MS1/MS2/MS3 pins:

| MS1  | MS2  | MS3  | Resolution |
| ---- | ---- | ---- | ---------- |
| LOW  | LOW  | LOW  | Full step  |
| HIGH | LOW  | LOW  | 1/2 step   |
| LOW  | HIGH | LOW  | 1/4 step   |
| HIGH | HIGH | LOW  | 1/8 step   |
| HIGH | HIGH | HIGH | 1/16 step  |

For **quieter operation** in a café, use 1/8 or 1/16 microstepping (wire MS pins to 3.3 V).
Trade-off: microstepping reduces max speed and torque slightly.

### 3.3 E-Stop Button (Optional but Recommended)

```
Raspberry Pi          E-Stop Button
──────────────        ──────────────
Pin 32 (GPIO 12) ──── One terminal
Pin 6  (GND)    ──── Other terminal
```

- The software uses an **internal pull-up** — no external resistor needed.
- When pressed, GPIO 12 goes LOW, triggering the safety latch.
- Configured as `active_low = true` in `doser_config.toml`.
- The E-stop latches until the next `begin()` (next dose cycle).

---

## 4. Complete Wiring Diagram

```
                   ┌──────────────────────┐
                   │    RASPBERRY PI       │
                   │                       │
    ┌──────────┐   │  3.3V (pin 1) ───────┼──→ HX711 VCC, A4988 VDD
    │ HX711    │   │                       │
    │          │   │  GPIO 5 (pin 29) ←────┼──── HX711 DT
    │  DT  ────┼───┤  GPIO 6 (pin 31) ────┼──→ HX711 SCK
    │  SCK ────┼───┤                       │
    │  VCC ────┼───┤  GPIO 23 (pin 16) ───┼──→ A4988 STEP
    │  GND ────┼─┐ │  GPIO 24 (pin 18) ───┼──→ A4988 DIR
    └──────────┘ │ │  GPIO 25 (pin 22) ───┼──→ A4988 EN (opt)
                 │ │  GPIO 12 (pin 32) ←──┼──── E-STOP (opt)
    ┌──────────┐ │ │                       │
    │ LOAD     │ │ │  GND (pin 6, etc) ───┼──┬── HX711 GND
    │ CELL     │ │ │                       │  ├── A4988 GND (logic)
    │  E+/E-   │ │ └───────────────────────┘  ├── 12V PSU (−)
    │  A+/A-   │ │                             └── E-STOP GND
    └────┬─────┘ │
         │       │    ┌──────────┐   ┌──────────────┐
         └───────┼──→ │ HX711    │   │ 12V PSU      │
                 └──→ │  (GND)   │   │  +  →  VMOT  │──→ A4988 VMOT
                      └──────────┘   │  −  →  GND   │──→ A4988 GND (motor)
                                     └──────────────┘
                                            ┌────────┐
                      A4988 1A/1B ─────────→│ NEMA17 │
                      A4988 2A/2B ─────────→│ MOTOR  │
                                            └────────┘
```

---

## 5. Load Cell Mounting

| Aspect          | Recommendation                                                            |
| --------------- | ------------------------------------------------------------------------- |
| **Orientation** | Arrow on the cell pointing **down** (toward gravity)                      |
| **Fixed end**   | Bolt to a rigid frame with M4/M5 screws                                   |
| **Free end**    | Attach your weighing platform or portafilter holder                       |
| **Vibration**   | Use rubber washers or grommets between the frame and motor                |
| **Leveling**    | Use a spirit level — even 1° tilt introduces systematic error             |
| **Max load**    | Choose a 5 kg cell for espresso (18 g target ≪ 5 kg = plenty of headroom) |

---

## 6. Software Configuration for Top Precision

The project ships with a tuned `doser_config.toml` optimized for ±0.1 g accuracy:

| Parameter                    | Value   | Why                                      |
| ---------------------------- | ------- | ---------------------------------------- |
| `filter.ma_window`           | 5       | Smooths sensor jitter                    |
| `filter.median_window`       | 5       | Removes spike outliers from vibration    |
| `filter.sample_rate_hz`      | 80      | HX711 high-speed mode (80 SPS)           |
| `control.epsilon_g`          | 0.02    | Stops within 0.02 g of target            |
| `control.hysteresis_g`       | 0.04    | Tight ±0.04 g settle band                |
| `control.stable_ms`          | 500     | 500 ms must be stable before "complete"  |
| `control.speed_bands`        | 4 bands | Gradual deceleration near target         |
| `predictor.enabled`          | true    | Anticipates momentum to reduce overshoot |
| `predictor.extra_latency_ms` | 30      | Compensates for filter + sensor lag      |
| `safety.max_overshoot_g`     | 0.5     | Aborts if >0.5 g over target             |

---

## 7. Calibration

### 7.1 Prepare Calibration Weights

You need certified weights (at least ±0.1 g accuracy):

- Minimum: 2 points (e.g. 0 g and 100 g)
- Recommended: 5+ points spanning your operating range

### 7.2 Create Calibration CSV

```csv
raw,grams
842913,0.0
861245,5.0
879577,10.0
897909,15.0
916241,20.0
934573,25.0
952905,50.0
```

To get the `raw` values, run a quick test read:

```bash
./target/release/doser_cli --config doser_config.toml self-check
```

The self-check prints the raw count from the HX711. Note the value with no weight (tare), then place each calibration weight and record the raw count.

### 7.3 Alternatively: Persisted Calibration in TOML

If you already know your gain and zero-counts, add to `doser_config.toml`:

```toml
[calibration]
gain_g_per_count = 0.0005492    # grams per ADC count
zero_counts = 842913            # raw count at 0 g (tare)
offset_g = 0.0                  # additive offset (usually 0)
```

### 7.4 Verification

After calibrating, place a known weight and run:

```bash
./target/release/doser_cli --config doser_config.toml dose --grams 18.0 --json
```

Check that `final_g` in the JSON output is within ±0.1 g of the known weight.

---

## 8. Smoke Test Procedure

Run these in order after wiring everything up:

### Step 1: Self-Check

```bash
./target/release/doser_cli --config doser_config.toml self-check
```

**Expected:** "OK" with a raw scale reading printed. If it fails:

- _"timeout"_: Check HX711 wiring (DT/SCK pins, power, ground).
- _"motor"_: Check STEP/DIR connections and 12 V supply.

### Step 2: Small Dose

```bash
./target/release/doser_cli --config doser_config.toml dose --grams 1.0
```

Watch the motor run briefly. Verify the weight increases on the load cell.

### Step 3: Full Dose

```bash
./target/release/doser_cli --config doser_config.toml dose --grams 18.0 --json
```

Review the JSON output for `final_g`, `duration_ms`, and any abort reasons.

### Step 4: E-Stop Test

If E-stop is wired, start a dose and press the button mid-way:

```bash
./target/release/doser_cli --config doser_config.toml dose --grams 50.0
# Press E-stop during the run → should abort immediately
```

---

## 9. Troubleshooting

| Symptom                              | Cause                                | Fix                                                           |
| ------------------------------------ | ------------------------------------ | ------------------------------------------------------------- |
| **"timeout waiting for sensor"**     | HX711 not sending data               | Check DT/SCK wires, ensure 3.3 V power, verify solder joints  |
| **Raw value stuck at 0**             | Load cell disconnected or wrong pins | Verify E+/E-/A+/A- wires match HX711 labels                   |
| **Weight reads negative**            | Load cell wired backwards            | Swap A+ and A- wires                                          |
| **Noisy readings (±1 g jitter)**     | EMI from motor wires                 | Separate load cell wires from motor wires; use shielded cable |
| **Motor vibrates but doesn't turn**  | Coil wires swapped                   | Swap one coil pair (e.g. swap 1A/1B)                          |
| **Motor overheats**                  | Current limit too high               | Reduce VREF on A4988 potentiometer                            |
| **Motor stalls (no-progress abort)** | Current limit too low or jam         | Increase VREF; check bean path for blockage                   |
| **Overshoot > 0.5 g**                | Speed too fast near target           | Lower `fine_speed` or add a slower speed band                 |
| **Dose too slow**                    | Speed bands too conservative         | Increase `coarse_speed` or widen first threshold              |
| **E-stop doesn't work**              | Wrong pin or `active_low` mismatch   | Check `estop_in` pin number; try `active_low = false`         |

---

## 10. Safety Warnings

1. **Mains voltage**: The 12 V supply plugs into mains power. Never open it. Use a certified adapter.
2. **Moving parts**: Keep hands away from the motor/auger while running.
3. **E-stop**: Always wire an E-stop for any unattended or commercial use.
4. **ESD**: Touch the Pi's ground before handling the HX711 — static can damage it.
5. **Hot components**: The A4988 and motor can get hot. Add heatsinks if operating continuously.
6. **Food safety**: If beans touch any 3D-printed parts, use food-safe filament (PETG or food-grade PLA).

---

## Appendix A: DRV8825 Alternative

The DRV8825 is a drop-in replacement for the A4988 with higher current (2.5 A vs 2 A) and up to 1/32 microstepping. The pinout is **identical** — just swap the board. Adjust current limit: `VREF = Imax / 2` (for 0.1 Ω sense resistors).

## Appendix B: Using Pi Zero 2 W

The Pi Zero 2 W ($15) works perfectly for this project:

- Same ARM64 architecture, same GPIO header.
- Lower power consumption (ideal for always-on in a café).
- Requires a micro-USB OTG adapter for initial setup, then SSH over Wi-Fi.
- Build performance is slower — cross-compile on a desktop for faster iteration:

```bash
# On your Mac/Linux desktop:
rustup target add aarch64-unknown-linux-gnu
cargo build -p doser_cli --features hardware --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/doser_cli pi@doser.local:~/
```

## Appendix C: Quick Precision Checklist

- [ ] Load cell mounted level with rigid frame
- [ ] HX711 powered from 3.3 V (not 5 V)
- [ ] Load cell wires short (< 30 cm) and separate from motor wires
- [ ] 100 µF capacitor on A4988 VMOT
- [ ] A4988 current limit set correctly for your motor
- [ ] `predictor.enabled = true` in config
- [ ] `epsilon_g = 0.02` (or lower) for tight stop
- [ ] Calibrated with ≥ 5 weight points spanning operating range
- [ ] Re-tare before each batch of doses
- [ ] Run 10-dose accuracy test to verify ±0.1 g

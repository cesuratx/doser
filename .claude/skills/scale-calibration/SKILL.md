---
name: scale-calibration
description: Calibrate the doser's HX711 load cell — turn raw ADC counts into grams using known reference weights and a strict raw,grams CSV. Use when asked to calibrate the scale, fix readings that are off, build a calibration file, or after replacing/re-wiring the load cell.
---

# Scale calibration (HX711 → grams)

Maps raw 24-bit HX711 counts to grams via an ordinary-least-squares fit
(`grams = a·raw + b`), persisted as a strict CSV the CLI loads with `--calibration`.

**Precondition:** the load cell must produce a *non-railed* reading that changes under load.
If it sits pinned at `0x800000`/`0x7FFFFF`, calibration is meaningless — fix wiring first
via the `hardware-bringup` skill / `docs/ops/HARDWARE_LESSONS.md`.

## 1. Gather raw counts at known weights
Use the probe to read stable raw counts. Place each known reference mass on the cell, let it
settle, record the median raw value:
```bash
# Empty pan (tare point) and several known weights spanning the working range:
HX711_STREAM=1 HX711_READS=50 cargo run -p doser_hardware --example hx711_probe --features hardware
```
Collect ≥3 points (more is better) across the range you actually dose in (e.g. 0 g, 5 g,
18 g, 50 g). Calibration weights should bracket your typical target.

## 2. Write the calibration CSV (strict header)
Header MUST be exactly `raw,grams`, on the **first line**. Raw values must be **strictly
monotonic** (increasing or decreasing), no duplicates, ≥2 rows:
```csv
raw,grams
123456,0.0
1543210,5.0
5123987,18.0
14002311,50.0
```

> **No comment lines.** The CSV reader has no comment character configured, so a leading `#`
> line is parsed as the header and the file is rejected with
> `Invalid headers in calibration CSV. Expected 'raw,grams'.`
> Record the date, cell and board in the commit message or in
> `docs/ops/HARDWARE_LESSONS.md` — not inside the CSV.

The OLS fit folds the intercept into a tare offset (counts), so you do not hand-tune zero.

## 3. Validate the fit
```bash
./target/release/doser_cli --config etc/doser_config.toml --calibration cal.csv health
```
`--calibration` is a **global** flag: it goes before the subcommand, as above.

> **A `[calibration]` table in the TOML config silently wins over `--calibration`.** The load
> order in `doser_cli/src/main.rs` takes the persisted TOML calibration if present and only
> falls back to the CSV otherwise — with no warning that your `--calibration` file was ignored.
> If a new CSV appears to change nothing, check the config first:
> ```bash
> grep -n '\[calibration\]' etc/doser_config.toml
> ```
> Remove or update that table to let the CSV take effect.

The fit fails loudly on degenerate input (zero/non-finite slope, non-monotonic raw). Then
sanity-check: put a known weight on and confirm reported grams matches within tolerance.

## 4. Verify accuracy across the range
Place 2–3 weights NOT used in the fit and confirm the error is within `epsilon_g`. Linearity
error usually means too few points or a load cell loaded off-axis. Re-take points if a
weight reads >1–2% off.

## 5. Record it
Commit the validated CSV (e.g. `etc/calibration.csv`), note the cell/board it was taken with,
the date, and the residual error in `docs/ops/HARDWARE_LESSONS.md`. Recalibrate after any
mechanical change to the cell, mount, or wiring. To make a fit permanent, promote it into the
config's `[calibration]` table (`PersistedCalibration`) — that is what runtime prefers, and per
step 3 it then overrides any `--calibration` CSV.

# Configuration Schema

Typed TOML loaded by `doser_config` (`doser_config/src/lib.rs`). Every key, default and
validation rule below was read off the `Deserialize`/`Default` impls and
`Config::validate()` in that file — not off a sample config.

Two sample configs ship with the repo and differ on purpose:

- `etc/doser_config.toml` — the config the CLI uses by default (`--config` defaults to it),
  tuned for the bench Pi (10 SPS HX711).
- `doser_config.toml` (repo root) — a precision-oriented example used by the README and the
  architecture docs; assumes an 80 SPS board.

## Required vs defaulted — read this first

`serde` only fills in a field when the type or the field carries a default. In this schema:

| Table | Table itself | Fields inside |
| --- | --- | --- |
| `[pins]` | **required** | `hx711_dt`, `hx711_sck`, `motor_step`, `motor_dir` **required**; `motor_en`, `estop_in` optional (`Option`) |
| `[filter]` | **required** | `ma_window`, `median_window`, `sample_rate_hz` **required**; `ema_alpha` optional |
| `[timeouts]` | **required** | `sample_ms` (alias `sensor_ms`) **required**; `settle_ms` optional and ignored |
| `[control]` | optional | all defaulted |
| `[safety]` | optional | all defaulted |
| `[logging]` | optional | all optional |
| `[hardware]` | optional | defaulted |
| `[predictor]` | optional | all defaulted |
| `[estop]` | optional | all defaulted |
| `[runner]` | optional | defaulted |
| `[calibration]` | optional | `gain_g_per_count`, `zero_counts` **required if the table is present**; `offset_g` defaults to 0.0 |

Omitting a required field is a **parse error**, not a fallback. The smallest config that
loads is:

```toml
[pins]
hx711_dt = 5
hx711_sck = 6
motor_step = 23
motor_dir = 24

[filter]
ma_window = 4
median_window = 3
sample_rate_hz = 50

[timeouts]
sample_ms = 100
```

Anything less fails at parse time with `missing field "filter"` (or `"timeouts"`, `"pins"`).

## Table of Contents

- [pins](#pins)
- [filter](#filter)
- [control](#control)
- [timeouts](#timeouts)
- [safety](#safety)
- [logging](#logging)
- [hardware](#hardware)
- [predictor](#predictor)
- [estop](#estop)
- [runner](#runner)
- [calibration (persisted)](#calibration-persisted)
- [calibration CSV](#calibration-csv)

## [pins]

Table required. BCM (GPIO) numbering. Only read on hardware builds
(`--features hardware` on Linux); simulation builds parse but ignore them.

| Key | Type | Required | Notes |
| --- | --- | --- | --- |
| `hx711_dt` | u8 | yes | HX711 data line (input, goes low when a sample is ready) |
| `hx711_sck` | u8 | yes | HX711 clock line (output) |
| `motor_step` | u8 | yes | STEP pulse to the stepper driver |
| `motor_dir` | u8 | yes | DIR level |
| `motor_en` | u8 | no | Enable pin, treated as **active-low** (low = enabled) |
| `estop_in` | u8 | no | E-stop input; when absent no E-stop checker is installed |

## [filter]

Table required.

| Key | Type | Required | Default | Validation |
| --- | --- | --- | --- | --- |
| `ma_window` | usize | yes | — | `1..=10_000` |
| `median_window` | usize | yes | — | `1..=10_000` |
| `sample_rate_hz` | u32 | yes | — | `> 0` |
| `ema_alpha` | f32 | no | absent = EMA disabled | when set, `(0.0, 1.0]` |

Semantics: median prefilter → moving average → optional EMA. `sample_rate_hz` paces sampling
in simulation; on hardware builds sampling is event-driven off HX711 data-ready and this
value is used for pacing/telemetry and by `monitor` as its default poll rate.

## [control]

Table optional; every key is defaulted from `ControlCfg::default()`.

| Key | Type | Default | Validation |
| --- | --- | --- | --- |
| `coarse_speed` | u32 | 1200 | `> 0` |
| `fine_speed` | u32 | 250 | `> 0` |
| `slow_at_g` | f32 | 1.0 | finite, `>= 0` |
| `hysteresis_g` | f32 | **0.05** | finite, `>= 0` |
| `stable_ms` | u64 | 250 | `<= 300_000` (5 min) |
| `epsilon_g` | f32 | **0.0** | finite, `[0.0, 1.0]` |
| `speed_bands` | array | `[]` (empty) | each threshold finite `>= 0`, each `sps > 0` |

> The 0.07 / 0.08 values you may have seen documented as "defaults" are simply what
> `etc/doser_config.toml` sets. The code defaults are 0.05 and 0.0.

`speed_bands` accepts either shape:

```toml
[control]
# array of tables
speed_bands = [
    { threshold_g = 1.0, sps = 1100 },
    { threshold_g = 0.5, sps = 450 },
]
# ...or array of tuples, equivalent
# speed_bands = [[1.0, 1100], [0.5, 450]]
```

Semantics:

- Coarse/fine speed selection by error magnitude vs `slow_at_g`; `speed_bands`, when
  non-empty, refines that into a step table keyed on remaining grams.
- Completion uses the asymmetric stop threshold `w + epsilon_g >= target`.
- Settle requires the weight to hold the completion zone for `stable_ms` after the motor
  stops. A reading that leaves the zone downward clears the settle timer and restarts the
  motor to top the dose up.
- A reading *above* the acceptance band `max(epsilon_g, hysteresis_g)` does **not** reset the
  settle timer. Over-delivery is irreversible — the auger cannot remove mass — so a dose that
  lands above the band but at or below `target + safety.max_overshoot_g` settles and
  completes. `max_overshoot_g` is the knob that decides how much over-delivery is acceptable;
  anything beyond it aborts `Overshoot`. (Before the audit remediation such a dose spun until
  `safety.max_run_ms` and then reported `MaxRuntime`, which was both a hang and a wrong
  diagnosis.)
- **Caveat: `hysteresis_g` currently has no effect on completion.** The completion zone opens
  at `target - epsilon_g` while the band opens at `target - max(epsilon_g, hysteresis_g)`,
  which is at or below it, so no in-zone reading is ever below the band and the only branch
  that consults the band is unreachable. Tune completion with `epsilon_g` and
  `safety.max_overshoot_g`. Making `hysteresis_g` meaningful needs a settle test based on the
  weight having stopped *changing* rather than sitting near the target — a control-law change,
  not a config change.
- **`control.stable_ms` is the settle knob.** There is no settle knob under `[timeouts]`
  (see below).

## [timeouts]

Table required.

| Key | Type | Required | Validation |
| --- | --- | --- | --- |
| `sample_ms` | u64 | yes | `>= 1` |
| `settle_ms` | u64 | no | parsed, **ignored** |

- `sample_ms` also accepts the alias **`sensor_ms`** (`etc/doser_config.toml` uses the
  alias). Write one or the other, not both.
- It is the per-read timeout. It must comfortably exceed one sample period, or every read
  times out: at 10 SPS a sample arrives every ~90–100 ms, so 200 ms is a sane value; at
  80 SPS, ~100 ms is fine.
- **`settle_ms` is deprecated and does nothing.** It exists only so older configs that put a
  settle window under `[timeouts]` still parse. Tune `control.stable_ms` instead.

## [safety]

Table optional; defaults come from `doser_config::Safety::default()`.

| Key | Type | Config default | Effective CLI default | Validation |
| --- | --- | --- | --- | --- |
| `max_run_ms` | u64 | 0 | **60_000** | — |
| `max_overshoot_g` | f32 | 0.0 | **2.0** | finite, `>= 0` |
| `no_progress_epsilon_g` | f32 | 0.02 | 0.02 | finite, `(0.0, 1.0]` |
| `no_progress_ms` | u64 | 1200 | 1200 | `>= 1`, `<= 86_400_000` |

The two-column split matters: `doser_config` defaults `max_run_ms`/`max_overshoot_g` to
zero, and `doser_cli` treats a zero as "unset" and substitutes
`doser_core::SafetyCfg::default()` (60 s / 2.0 g) before building the doser
(`doser_cli/src/dose.rs`). So a config that omits `[safety]` still runs with a 60 s cap and a
2 g overshoot guard. Explicit `--max-run-ms` / `--max-overshoot-g` on `dose` take precedence
over both.

Semantics:

- Max-runtime guard: abort once the run exceeds `max_run_ms`.
- Overshoot guard: abort once weight exceeds target by more than `max_overshoot_g`.
- No-progress watchdog: abort if weight changes by less than `no_progress_epsilon_g` for
  `no_progress_ms` **while the motor is commanded to run**. Time spent with the motor
  deliberately stopped (settling, or a predictor early stop) does not count against it, and
  the deadline is re-armed when the motor is restarted after a dip below target.
- E-stop: debounced and latched until the next `begin()`.

## [logging]

Table optional; all keys optional.

| Key | Type | Notes |
| --- | --- | --- |
| `file` | string | path to a log file; enables a non-blocking file sink |
| `rotation` | string | `"never"` (default), `"daily"`, `"hourly"` |
| `level` | string | **parsed but ignored** — see below |

- Console level comes from the CLI flag `--log-level` or from `RUST_LOG`; `RUST_LOG` wins
  when set. `logging.level` in the TOML is deserialized and never read by the CLI.
- All log records — pretty *and* JSON — are written to **stderr**. stdout carries only the
  CLI's own output (the `--json` result line, `final: X.XX g`, `Detected HX711 rate: …`,
  monitor/jog status lines).
- When `file` is set, a `WorkerGuard` is held for the process lifetime so the appender flushes.

## [hardware]

Table optional.

| Key | Type | Default | Validation |
| --- | --- | --- | --- |
| `sensor_read_timeout_ms` | u64 | 150 | `>= 1` |

Max time to wait for HX711 data-ready before failing a read. The `monitor` subcommand uses
`max(sensor_read_timeout_ms, 200)` as its per-read timeout.

## [predictor]

Table optional.

| Key | Type | Default | Validation |
| --- | --- | --- | --- |
| `enabled` | bool | false | — |
| `window` | usize | 6 | `1..=10_000` |
| `extra_latency_ms` | u64 | 20 | — |
| `min_progress_ratio` | f32 | 0.10 | finite, `[0.0, 1.0]` |

When enabled, the core keeps a rolling slope estimate and predicts in-flight grams using the
configured extra latency. If the predicted final mass (current + in-flight + epsilon) would
cross target, the motor is stopped early to reduce overshoot. Activation is gated until at
least `min_progress_ratio` of target is reached, so startup noise cannot trigger it. See
[ADR-001](../adr/ADR-001-predictive-stop.md).

## [estop]

Table optional.

| Key | Type | Default | Validation |
| --- | --- | --- | --- |
| `active_low` | bool | true | — |
| `debounce_n` | u8 | 2 | `>= 1` |
| `poll_ms` | u64 | 5 | `>= 1` |

- `debounce_n` is passed to the core on **every** build; `active_low` and `poll_ms` are only
  used by the GPIO-backed checker, which exists on hardware builds with `pins.estop_in` set.
- With a normally-open button to GND, keep `active_low = true`. For a fail-safe wiring, use a
  normally-closed button and `active_low = false` so a cut wire also trips the stop.

## [runner]

Table optional.

| Key | Type | Default | Values |
| --- | --- | --- | --- |
| `mode` | string | `"sampler"` | `"sampler"` \| `"direct"` |

- `sampler` (default): a background sampler thread feeds the control loop; on hardware
  builds it is event-driven off HX711 data-ready.
- `direct`: the control loop reads the scale inline.
- `dose --direct` forces direct mode regardless of this setting. Unknown values are rejected
  by serde at parse time.

## [calibration] (persisted)

Table optional. **When present it is preferred over `--calibration <CSV>` at runtime** — the
CLI only loads the CSV if this table is absent (`doser_cli/src/main.rs`).

| Key | Type | Required | Default |
| --- | --- | --- | --- |
| `gain_g_per_count` | f32 | yes | — |
| `zero_counts` | i32 | yes | — |
| `offset_g` | f32 | no | 0.0 |

```toml
[calibration]
gain_g_per_count = 0.0005492   # grams per ADC count
zero_counts = 842913           # raw count at 0 g (tare)
offset_g = 0.0                 # additive grams offset, rarely needed
```

Conversion: `grams = gain_g_per_count * (raw - zero_counts) + offset_g`. With no calibration
at all the core defaults to `gain_g_per_count = 0.01`, `zero_counts = 0` — which matches the
simulator's centigram counts but is meaningless on real hardware.

## Calibration CSV

- Strict header, exactly `raw,grams`. **No comment syntax exists** — the CSV reader is built
  without a comment character, so a `#` line is parsed as data (or as the header) and the
  file is rejected. Keep the file to a header plus rows.
- At least 2 rows; raw values must be strictly monotonic (no duplicates, no zig-zag).
- At most 100,000 rows (memory guard).
- OLS fit across all rows computes `grams = a*raw + b`; the core uses `scale_factor = a` and
  `offset` as tare counts `round(-b/a)`.

Outlier handling (robust refit):

- After the initial OLS fit, the RMS residual is computed. Points with `|residual| > 2×RMS`
  are excluded from a one-pass refit using numerically stable online covariance updates.
- If fewer than 2 inliers remain, or X variance is degenerate, the initial fit is kept.
- A zero slope is rejected: raw must vary and map to varying grams.

`doser_config.csv` in the repo root is a working example in this schema.

## Validation

`Config::validate()` runs after parsing and reports the first violation with the offending
key name. Beyond the per-key rules above it rejects non-finite (NaN/±Inf) floats and caps
filter/predictor window sizes at 10,000 entries as a heap-exhaustion guard. The CLI also
refuses config files larger than 1 MiB.

To check a config without touching the mechanism, run any subcommand — load and validation
happen before dispatch:

```bash
cargo run -q -p doser_cli -- --config etc/doser_config.toml self-check
```

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Graceful shutdown handling (SIGTERM/SIGINT) for safe process termination
- Health check command (`doser_cli health`) for operational monitoring
- `monitor` subcommand — a live weight web UI (self-contained page, no CDN) with
  `GET /reading` polling and browser tare. Unauthenticated by design for bench
  use: `--bind` defaults to `0.0.0.0` and the CLI now prints an explicit
  `WARNING:` on stderr whenever the bind address is not loopback
- `motor` subcommand — a fixed-rate jog for hardware bring-up (no scale, no
  control loop): `--sps`, `--ms` / `--steps`, `--dir cw|ccw`, Ctrl-C safe
- Three runnable examples under `doser_cli/examples/` (`quick_start`,
  `simulated_hardware`, `custom_strategy`), rewritten against the current APIs and
  now built by CI's `--all-targets`
- Comprehensive business and best practices review documentation
- MIT and Apache-2.0 dual licensing
- API stability notices and safety disclaimers in README
- `doser_hardware::sim_pair()` — linked simulated scale/motor sharing per-instance
  state (replaces process-global statics; isolates parallel simulations)
- Regression tests asserting the motor is stopped on every safety abort
  (overshoot, max-runtime, no-progress, E-stop)

### Fixed

- **Settle livelock on over-delivery (critical):** a dose that landed above the
  acceptance band `max(epsilon_g, hysteresis_g)` but within
  `safety.max_overshoot_g` never completed. The settle timer was restarted by any
  out-of-band reading, but the completion-zone branch stops the motor and returns
  before the motor-command section, so nothing could bring a high reading back
  down: the timer reset on every subsequent sample, the run burned the whole
  `safety.max_run_ms`, and a finished, slightly over-delivered dose was reported as
  `MaxRuntime`. Beans still in flight when the motor stops make this the ordinary
  case on real hardware. Only a *low* excursion restarts the timer now; a dip out
  of the completion zone still clears it and restarts the motor. Reproduced from
  the CLI (the sim dose failed ~1 run in 3 before the fix, 10/10 after) and pinned
  by `doser_core/tests/hardening.rs::persistent_over_delivery_completes_instead_of_livelocking`
  and `doser_core/tests/doser.rs::settling_above_the_band_but_within_overshoot_completes`.
  Note this makes `control.hysteresis_g` provably inert — see CONFIG_SCHEMA.md.
- **Calibration precision (critical):** the calibration gain was quantized to an
  integer centigrams-per-count, collapsing realistic load-cell gains (e.g.
  ~0.0005 g/count) to zero so the scale read ~0 g on real hardware. Gain is now
  stored as a scaled integer (`fixed_point::GAIN_SCALE`) preserving sub-count
  resolution while keeping the per-sample math integer/deterministic.
- **Persisted calibration `offset_g`** was silently dropped when loading from TOML;
  it is now preserved end-to-end.
- **Ctrl-C/shutdown** is now honored in the default (non-stats) runner path; the
  motor is stopped and the run aborts instead of running to completion.
- **Motor-stop on abort paths** now retries best-effort and escalates to an
  error-level log on persistent failure instead of silently ignoring the error.
- **E-stop responsiveness in sampler mode** is decoupled from sensor read latency
  via an out-of-band poll each orchestration iteration.
- **E-stop GPIO checker thread** no longer leaks: it self-terminates (via a `Weak`
  ref) when the checker is dropped, releasing the GPIO claim.
- **Memory ordering:** the sampler stall-watchdog timestamp and the hardware motor
  running/speed flags use Release/Acquire instead of Relaxed for reliable
  cross-thread visibility of safety-relevant state.
- **HX711 SCK timing:** clock edges now busy-wait against a monotonic `Instant`
  deadline (was a `spin_loop`-based *calibrated spin count*, which on the Pi 5's
  RP1 GPIO produced sub-microsecond, jittery pulses and unstable garbage reads —
  `spin_loop()` is ~1 cycle on aarch64, so the calibration under-counted). A
  calibrated spin count is explicitly **not** the pattern to use here; see
  HARDWARE_LESSONS Lesson 1 and the rule in CLAUDE.md.
- Thread leak in background sampler (issue #1.2) - sampler threads now exit promptly on drop
- Optimized sampler shutdown to <200ms using lock-free AtomicBool
- Privilege escalation risk in real-time setup (issue #1.1) - improved error handling and privilege checks
- Division by zero vulnerability in calibration loader (issue #1.3) - added validation
- Release workflow referenced a non-existent `doser` binary (the package builds
  `doser_cli`); release tarballs now ship the correct binary plus a `.sha256`.
- **Release artifacts for the Raspberry Pi were simulation-only.** The
  `aarch64-unknown-linux-gnu` build is now made `--features hardware`, so the
  published Pi tarball actually drives the HX711 and the motor instead of
  reporting successful-looking doses with no I/O. The x86_64-linux and macOS
  artifacts stay simulation-only on purpose (no GPIO on those hosts).
- **stdout/stderr contract.** Every log record — pretty *and* JSON — now goes to
  stderr; stdout carries only the CLI's own output (the `--json` result line,
  `final: X.XX g`, `Detected HX711 rate: …`, monitor/jog status lines). Previously
  a JSON log event carrying `final_g` was indistinguishable on stdout from the
  real result line. `doser_cli --json … dose` now emits exactly one object on
  stdout.
- **Safety watchdogs on the `--stats` path.** `dose` no longer carries its own
  copies of the control loop: both the plain and the `--stats` path go through
  `doser_core::runner::run_observed`, so max-runtime, overshoot, no-progress and
  E-stop behave identically with and without `--stats`. `slope_ema`, `stop_at_g`
  and `coast_comp_g` are now populated in `--json` output on every path instead of
  being null unless `--stats` was passed.
- **Motor is restarted after a dip.** `motor_stop`/`motor_stop_best_effort` now
  clear the started flag, so a reading that falls back below `target - epsilon`
  (settle-band dip, or a predictor early stop that plateaus short) re-issues
  `motor.start()` and tops the dose up, instead of commanding speeds at a stopped
  motor until the no-progress watchdog fires. The no-progress deadline is re-armed
  at the restart, since that watchdog measures time with the motor *commanded to
  run*.
- **`motor --sps` is validated up front.** The rate is range-checked by clap
  against `1..=doser_hardware::MAX_STEP_RATE_SPS` (5000, the clamp both backends
  apply), so an out-of-range rate exits 2 with a clear message instead of being
  silently clamped; `--sps 0` and its division-by-zero path are gone. `--steps` now
  rounds the derived duration up rather than truncating, and derives it from the
  clamped rate.
- CI ran on an unpinned `stable` toolchain (the toolchain action exported
  `RUSTUP_TOOLCHAIN` and overrode the 1.96.0 pin in `rust-toolchain.toml`); every
  job now resolves the pinned toolchain and echoes it. `master` was also missing
  from the push triggers, so mainline pushes were not built at all.
- `install.sh`'s systemd unit passed no subcommand, so the service exited 2 and
  crash-looped under `Restart=always`. The unit now runs `monitor` bound to
  `127.0.0.1:8080` (`DOSER_SERVICE_BIND`), uses `Restart=on-failure` with a start
  rate limit, is hardened, and is installed **disabled** — the installer no longer
  enables or starts a network listener on your behalf. It downloads the real
  release tarball for the detected target triple and no longer fetches config
  files over the network.
- The example `doser_config.csv` was not a calibration file at all; it is now a
  valid `raw,grams` file (tare plus 10/20/50/100/200 g points).

### Changed

- CI jobs consolidated: `checks` is the sole owner of fmt + clippy, the redundant
  `lint` job and ci.yml's duplicate `security` job are gone (security.yml's
  `cargo-audit` owns that). **The check names `lint` and `security-audit` no
  longer exist** — remove them from branch protection if they are required there.
  `checks`, `test`, `test-hardware-feature` and `coverage` keep their names.
- The tarpaulin `coverage` job is explicitly informational: no threshold, gates
  nothing, and non-blocking on pull requests.
- `etc/doser_config.toml` sets `control.stable_ms = 800` in place of the
  no-op `timeouts.settle_ms = 800`.
- Improved error messages with actionable troubleshooting guidance
- Enhanced RT mode setup with better fallback behavior
- `control.hysteresis_g` is wired into the acceptance band `max(epsilon,
  hysteresis)` used by the settle check (previously parsed but unused). See the
  settle-livelock entry under Fixed above: the band's only branch turns out to be
  unreachable, so `hysteresis_g` still has no effect on completion — tune with
  `epsilon_g` and `safety.max_overshoot_g` instead
- Median prefilter uses O(n) selection (`select_nth_unstable`) instead of a full
  sort per sample, reducing control-loop jitter
- `Scale::read` documented as returning raw ADC counts (calibration converts to
  grams), correcting a misleading "centigrams" claim

### Documentation

- `docs/reference/CONFIG_SCHEMA.md` rewritten from the actual types and `Default`
  impls: required vs defaulted is now stated per key (omitting `[pins]`,
  `[filter]` or `[timeouts]` is a parse error, not a fallback), the previously
  undocumented `[estop]`, `[runner]`, `[calibration]`, `control.speed_bands`,
  `filter.ema_alpha` and the `sensor_ms` alias are covered, and the wrong
  `hysteresis_g`/`epsilon_g`/`max_run_ms`/`max_overshoot_g` defaults are corrected.
- `timeouts.settle_ms` documented as parsed-and-ignored (the settle knob is
  `control.stable_ms`) and removed from `etc/doser_config.toml`, which now sets
  `control.stable_ms` instead. `logging.level` likewise documented as ignored.
- `self-check` is described correctly everywhere: it reads the scale for one
  second and prints `Detected HX711 rate: {10|80} SPS`; it never touches the motor
  and never prints `OK`. That behavior belongs to `health`.
- Fixed CLI invocations in README/docs that omitted the `dose` subcommand or
  invoked a `doser` binary that is not built.
- Operator documentation added for `monitor` (LAN exposure, the
  `X-Doser-Monitor` header required on `POST /tare` and `/tare/clear`, the 409
  before the first reading) and for `motor`.
- `docs/concepts/build-ci.md` and `docs/testing/Strategy.md` now state what CI
  actually runs; fuzzing and the Criterion benches are local-only manual steps.
- The changelog no longer describes the HX711 SCK fix as a "calibrated busy-wait",
  the exact pattern HARDWARE_LESSONS Lesson 1 bans.

### Security

- The monitor's state-changing endpoints (`POST /tare`, `POST /tare/clear`) now
  require the custom `X-Doser-Monitor` header, which forces a CORS preflight that
  the server never satisfies — a hostile page in the operator's browser can no
  longer zero the scale mid-dose. They additionally require a LAN-looking `Host`
  (DNS-rebinding defence). Reads are unchanged.
- Config validation now rejects non-finite (NaN/±Inf) float fields and caps
  filter/predictor window sizes (heap-exhaustion guard)
- Config file size and calibration CSV row count are now bounded
- `install.sh` fails fast, quotes paths, and verifies a SHA-256 checksum before
  installing the binary; `cross` and the Rust toolchain are pinned for
  reproducible builds. The `yourdomain.com` placeholder origin is gone: the
  default source is this project's GitHub Releases, and `DOSER_SHA256` remains the
  out-of-band integrity path (a same-origin `.sha256` gives integrity, not
  authenticity)
- All three GitHub Actions workflows declare a least-privilege `permissions`
  block; write scopes are granted per-job only where needed

## [0.1.0] - 2025-XX-XX

### Added

- Initial release with HX711 scale and stepper motor support
- Hardware abstraction via traits (Scale, Motor, Clock)
- Type-state builder pattern for compile-time safety
- Comprehensive safety features:
  - Emergency stop (E-stop) with debouncing
  - Max runtime watchdog
  - Overshoot detection and abort
  - No-progress watchdog
- Calibration support via CSV with robust outlier rejection
- Filtering: median + moving average + optional EMA
- Control strategies: speed bands, hysteresis, settle time
- Early-stop predictor for reduced overshoot
- Simulation mode for hardware-free testing
- Real-time mode support (Linux: SCHED_FIFO, mlockall, affinity; macOS: mlockall only)
- Structured logging with tracing and optional JSON output
- CLI with commands: `dose`, `self-check` (`health`, `monitor` and `motor` were
  added later — see Unreleased)
- Comprehensive test suite:
  - 79+ unit and integration tests
  - Property-based tests with proptest
  - Fuzz testing with cargo-fuzz
  - Benchmarks with criterion
- Documentation:
  - Architecture overview with diagrams
  - Rust primer for newcomers
  - Operations runbook
  - Contributing guidelines
- Systemd service integration with log rotation
- Non-root deployment with udev rules for GPIO access

### Security

- Security and performance review completed (see docs/security-performance-review.md)
- Fixed sampler thread resource leak
- Safe privilege handling for real-time mode

[Unreleased]: https://github.com/cesuratx/doser/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/cesuratx/doser/releases/tag/v0.1.0

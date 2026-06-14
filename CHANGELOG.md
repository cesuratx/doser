# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Graceful shutdown handling (SIGTERM/SIGINT) for safe process termination
- Health check command (`doser health`) for operational monitoring
- Comprehensive business and best practices review documentation
- MIT and Apache-2.0 dual licensing
- API stability notices and safety disclaimers in README
- `doser_hardware::sim_pair()` — linked simulated scale/motor sharing per-instance
  state (replaces process-global statics; isolates parallel simulations)
- Regression tests asserting the motor is stopped on every safety abort
  (overshoot, max-runtime, no-progress, E-stop)

### Fixed

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
- **HX711 SCK timing:** clock edges now use a calibrated ~1µs busy-wait (was a
  no-op `spin_loop`, below the device's ~0.2µs minimum).
- Thread leak in background sampler (issue #1.2) - sampler threads now exit promptly on drop
- Optimized sampler shutdown to <200ms using lock-free AtomicBool
- Privilege escalation risk in real-time setup (issue #1.1) - improved error handling and privilege checks
- Division by zero vulnerability in calibration loader (issue #1.3) - added validation
- Release workflow referenced a non-existent `doser` binary (the package builds
  `doser_cli`); release tarballs now ship the correct binary plus a `.sha256`.

### Changed

- Improved error messages with actionable troubleshooting guidance
- Enhanced RT mode setup with better fallback behavior
- `control.hysteresis_g` is now implemented: the weight must stay within the
  acceptance band (`max(epsilon, hysteresis)`) for `stable_ms` to settle, so a
  noisy out-of-band reading resets the settle timer (previously parsed but unused)
- Median prefilter uses O(n) selection (`select_nth_unstable`) instead of a full
  sort per sample, reducing control-loop jitter
- `Scale::read` documented as returning raw ADC counts (calibration converts to
  grams), correcting a misleading "centigrams" claim

### Security

- Config validation now rejects non-finite (NaN/±Inf) float fields and caps
  filter/predictor window sizes (heap-exhaustion guard)
- Config file size and calibration CSV row count are now bounded
- `install.sh` fails fast, quotes paths, and verifies a SHA-256 checksum before
  installing the binary; `cross` and the Rust toolchain are pinned for
  reproducible builds

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
- CLI with commands: `dose`, `self-check`
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

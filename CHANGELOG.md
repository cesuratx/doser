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

### Fixed

- Thread leak in background sampler (issue #1.2) - sampler threads now exit promptly on drop
- Optimized sampler shutdown to <200ms using lock-free AtomicBool
- Privilege escalation risk in real-time setup (issue #1.1) - improved error handling and privilege checks
- Division by zero vulnerability in calibration loader (issue #1.3) - added validation

### Changed

- Improved error messages with actionable troubleshooting guidance
- Enhanced RT mode setup with better fallback behavior

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

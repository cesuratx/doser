# Testing Strategy

## Layers that run in CI

- Unit tests in each crate for pure logic (filters, predictor, conversions, calibration math,
  the monitor's JSON/CSRF/host helpers).
- Integration tests for runner/control paths and the CLI JSONL schema
  (`doser_cli/tests/`, `doser_core/tests/`).
- Property tests (proptest) for config validation and numeric invariants.
- Compile-check of the `hardware` feature (`test-hardware-feature` job). CI has no Pi, so the
  real GPIO/HX711 paths are never executed there — they are exercised by the on-Pi smoke test
  in [PI_SMOKE.md](../reference/PI_SMOKE.md).

Run them all with `cargo test` (simulation backends, works on any host).

## Local-only, manual layers

These exist in the repo but **no workflow runs them**; do not assume they gate a change.

- **Fuzzing** — `fuzz/fuzz_targets/fuzz_config_loader.rs` drives `doser_config::load_toml`
  followed by `Config::validate()`, asserting neither panics. Requires `cargo-fuzz` and a
  nightly toolchain (the workspace is pinned to stable 1.96.0):

  ```bash
  cargo install cargo-fuzz
  cargo +nightly fuzz run fuzz_config_loader
  ```

- **Benches** — `doser_core/benches/predictor.rs` (Criterion, `harness = false`):

  ```bash
  cargo bench -p doser_core
  ```

  `cargo test -p doser_core` also builds this target and runs it in Criterion's test mode
  (one iteration per benchmark), which proves it compiles and runs but measures nothing.

## Coverage

- `cargo tarpaulin` runs in CI and uploads an artifact, but it is informational: no
  threshold, gates nothing, `continue-on-error` on PRs, and it runs
  `--no-default-features` so hardware-gated code is excluded and the number understates
  real coverage.

## Guidelines

- No `unwrap`/`expect` outside tests (clippy denies it in release).
- Prefer the deterministic `TestClock` over sleeping.
- Assert JSONL keys and types, not formatting.
- Assert operator messages on **stderr** and result lines on **stdout** — all log records go
  to stderr, so a test that greps stdout for a log message will fail.
- A test that POSTs to the monitor's `/tare` endpoints must send the `X-Doser-Monitor` header
  (else 403) and must do so after at least one successful sample (else 409).

# Build, CI, Coverage

Toolchain is pinned by `rust-toolchain.toml`; every CI job installs it with
`rustup toolchain install --no-self-update || rustup show` and echoes
`rustup show active-toolchain`, so the resolved version is auditable in the log.

## What CI actually runs

`.github/workflows/ci.yml` — on push to `master`/`main`/`dev`/`release-*` and on PRs:

| Job | Steps |
| --- | --- |
| `checks` | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo check --workspace --all-targets`; `cargo check -p doser_hardware --no-default-features --features hardware`; `cargo hack check --each-feature` |
| `test` | build + test with simulation backends, and again with default features |
| `test-hardware-feature` | compile-check of the `hardware` feature (compile only — no Pi in CI) |
| `coverage` | `cargo tarpaulin`, uploaded as an artifact |

`.github/workflows/security.yml` — daily cron plus push/PR: `cargo-audit` (job name
`cargo-audit`) and `cargo-deny` against `deny.toml`.

`.github/workflows/release.yml` — on version tags: builds four targets. Only
`aarch64-unknown-linux-gnu` (the Raspberry Pi artifact) is built `--features hardware`; the
x86_64-linux and macOS artifacts are simulation-only builds, since there is no GPIO on those
hosts.

Notes on `coverage`: it is **informational only**. There is no threshold, it gates nothing,
and it is `continue-on-error` on pull requests. It runs with `--no-default-features`, so the
number understates real coverage.

## What is NOT run by CI

- **Fuzzing.** `fuzz/` holds a `cargo-fuzz` target (`fuzz_config_loader`, which drives
  `doser_config::load_toml`). No workflow invokes it — it is a local, manual step:

  ```bash
  cargo install cargo-fuzz
  cargo +nightly fuzz run fuzz_config_loader
  ```

  (`cargo-fuzz` needs a nightly toolchain; the workspace pin is stable.)

- **Benchmarks.** `doser_core/benches/predictor.rs` is a Criterion bench, declared in
  `doser_core/Cargo.toml` with `harness = false` so Criterion's own harness runs. No workflow
  invokes it; run it locally:

  ```bash
  cargo bench -p doser_core
  ```

  Note that `cargo test -p doser_core` also builds and runs this target in Criterion's test
  mode (one iteration per benchmark), which is a quick smoke test but measures nothing.

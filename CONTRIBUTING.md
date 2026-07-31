# Contributing to Doser

Thank you for your interest in contributing! Please follow these guidelines.

## How to Contribute

- Fork the repository and create your branch from `master` (the default branch).
- Write clear, idiomatic Rust code and document public items with Rustdoc.
- Add or update tests for new features or bug fixes.
- Run the checks below before submitting a PR.
- Submit a pull request with a clear description of your changes.

## Local checks

These are the same invocations CI runs, so a clean local run means a clean `checks` job:

```bash
cargo fmt --all -- --check                    # CI fails on any diff
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                        # simulation backends; runs on any host
```

Real-hardware code lives behind the `hardware` cargo feature, which pulls in `rppal` and is
Linux-only. On a non-Linux host, compile-check it by cross-targeting rather than skipping it:

```bash
rustup target add x86_64-unknown-linux-gnu
cargo check --target x86_64-unknown-linux-gnu -p doser_hardware --no-default-features --features hardware
cargo check --target x86_64-unknown-linux-gnu -p doser_cli       --no-default-features --features hardware
```

The toolchain is pinned in `rust-toolchain.toml`; don't override it when reproducing a CI failure.

## Running it

The `doser_cli` package produces a binary named **`doser_cli`**, not `doser`:

```bash
# Simulation (no GPIO). DOSER_TEST_SIM_INC makes the simulated scale advance per read;
# without it the weight never moves and the dose aborts on `max_run_ms`.
DOSER_TEST_SIM_INC=0.01 cargo run -p doser_cli -- --config ./doser_config.toml dose --grams 10

# Real Pi build
cargo build --release -p doser_cli --features hardware   # -> target/release/doser_cli
```

Subcommands: `dose`, `health`, `self-check`, `monitor`, `motor`. Global flags (`--config`,
`--calibration`, `--json`, `--log-level`) go **before** the subcommand. `--config` defaults to
`etc/doser_config.toml` (the Pi config); `./doser_config.toml` is the roomier sim-friendly one.

## Conventions

- **Errors:** libraries use `thiserror`; the CLI uses `eyre`/`color-eyre`. Don't `unwrap`/`expect`
  in non-test code — clippy denies `clippy::unwrap_used`/`expect_used` in release builds.
- **Design decisions** go in `docs/adr/` as ADRs.
- **Hardware lessons** go in `docs/ops/HARDWARE_LESSONS.md` (append-only, newest first). If you
  solve a hardware fault or hit non-obvious electrical/timing behavior, add a dated entry.
- Logs go to stderr; `--json` result lines go to stdout. Keep it that way so stdout stays pipeable.
- Match the surrounding code's style, comment density, and naming.

## Code of Conduct

All contributors are expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Issues

- Use GitHub Issues for bug reports and feature requests.
- Please provide as much detail as possible, including platform and hardware info.

## Review Process

- All PRs are expected to be reviewed and approved by a maintainer before merge.
- CI must be green before merging. Workflows that run on every PR:
  - **CI** (`.github/workflows/ci.yml`) — jobs `checks` (fmt, clippy, `cargo check`, `cargo hack
    check --each-feature`), `test`, `test-hardware-feature`, and `coverage`. `coverage` is
    informational and is non-blocking on pull requests; it enforces no threshold.
  - **Security** (`.github/workflows/security.yml`) — `cargo-audit` and `cargo-deny`.
- Third-party GitHub Actions are pinned to full commit SHAs. If you add or update one, pin the SHA
  and leave the human-readable version in a trailing comment; see the header comment in any
  workflow file for how to resolve a new pin.

---

Happy coding!

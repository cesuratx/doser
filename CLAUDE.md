# CLAUDE.md

Guidance for AI agents (and humans) working in this repo. Read this first.

## What this is

`doser` is a Rust workspace for a **precision coffee-bean dosing system on Raspberry Pi**
(HX711 load cell + stepper motor). It is the owner's long-lived **product-experimentation
platform** — the goal is the best hardware app we can build, with quality that **compounds
across sessions** rather than being re-derived each time. Optimize for durable engineering,
repository discipline, and accumulated hardware expertise.

## Workspace layout

| Crate | Responsibility |
| --- | --- |
| `doser_core` | Control loop, predictor, safety, filtering |
| `doser_hardware` | GPIO/HX711/motor drivers **and** simulation backends |
| `doser_config` | Typed config (TOML) + validation |
| `doser_traits` | `Scale`/`Motor`/`Clock` traits shared across crates |
| `doser_cli` | Binary `doser_cli`: `dose`, `health`, `self-check` |

**Feature gating:** real hardware is behind the `hardware` cargo feature (`rppal`, Linux
only). Default builds use the simulation backends so CI runs on x86. The `rt` feature adds
SCHED_FIFO / mlockall for low jitter.

## Build, test, run

```bash
cargo test                                            # sim backends, runs anywhere
cargo build --release -p doser_cli --features hardware # real Pi build → target/release/doser_cli
cargo clippy --all-targets -- -D warnings             # lints are denied in release
cargo fmt
```

On the Pi the binary is `target/release/doser_cli` (not `doser`). It needs GPIO access —
be in the `gpio` group (`/dev/gpiomem*`); no root required.

## Conventions

- **Errors:** libraries use `thiserror`; the CLI uses `eyre`/`color-eyre`. Don't `unwrap`/
  `expect` in non-test code — clippy denies it in release.
- **Design decisions** go in `docs/adr/` as ADRs. **Hardware lessons** go in
  `docs/ops/HARDWARE_LESSONS.md` (append-only, newest first).
- **Bit-bang timing** (SCK, STEP) must use an `Instant`-deadline busy-wait, never a
  calibrated spin count — see HARDWARE_LESSONS Lesson 1 (a real Pi 5 bug).
- Match the surrounding code's style, comment density, and naming.

## Memory & "don't reinvent the wheel" (important)

This project relies on persistent learning. **At the start of hardware work, recall what we
already know; at the end of a session with notable findings, write it back.**

- **Claude memory** lives under the session memory dir (indexed by `MEMORY.md`). Key notes:
  `project-vision`, `user-engineering-standards`, `hardware-bringup-procedure`,
  `pi5-gpio-timing-gotcha`, `hx711-current-state`.
- **In-repo durable knowledge:** `docs/ops/HARDWARE_LESSONS.md`, `docs/adr/`,
  `docs/guides/HARDWARE_SETUP.md`.

If you solve a hardware fault or hit a non-obvious electrical/timing behavior, add a dated
entry to HARDWARE_LESSONS.md **and** a memory note. Diagnose at signal level (datasheet
timing, rails, SPS, pull-vs-drive) and prefer evidence (probe output, measured timing) over
assertion.

## Hardware bring-up quickstart

- Real-backend smoke test: `doser_cli --config etc/doser_config.toml health`.
- Authoritative HX711 probe: `cargo run -p doser_hardware --example hx711_probe --features hardware`
  (env: `HX711_STREAM=1` for the live load test, `HX711_DT`/`HX711_SCK`/`HX711_HIGH_US`...).
- Pin states: `pinctrl get 5,6`. Note `pinctrl` pulses are too slow to validate fast
  clocking (they trip HX711 power-down) — use the probe.
- Current hardware status & gotchas: `docs/ops/HARDWARE_LESSONS.md`.

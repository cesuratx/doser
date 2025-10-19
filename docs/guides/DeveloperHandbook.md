# Developer Handbook (doser)

Welcome! This guide is for Java/C# engineers onboarding to the Rust-based "doser" project. It’s concise, repo-specific, and links to deeper pages.

- Repo root: `cesuratx/doser` (branch: release-25.9.1)
- Crates:
  - `doser_traits` — interfaces (Scale, Motor, Clock). Safe boundary.
  - `doser_core` — control loop, filters, predictor, runner. Hardware-agnostic.
  - `doser_hardware` — Raspberry Pi GPIO + HX711 + motor driver, plus a simulator.
  - `doser_config` — typed TOML config, calibration CSV parsing/robust refit.
  - `doser_cli` — CLI UX, tracing/logging, JSONL output, RT helpers.

## A. Quick Start Map

Reading order for Java/.NET devs:

1. Concepts primer linked to real code:

- Ownership & borrowing → docs/concepts/ownership-borrowing.md
- Traits/generics/trait objects → docs/concepts/traits-generics.md
- Enums/matching/Result/Option → docs/concepts/enums-matching.md
- Error handling (thiserror + eyre) → docs/concepts/error-handling.md
- Time & determinism → docs/concepts/time.md
- Concurrency (channels) → docs/concepts/concurrency.md
- Fixed-point math & filters → docs/concepts/fixed-point-and-filters.md
- Control loop & state machine → docs/concepts/control-loop.md
- Config parsing → docs/concepts/config.md
- Logging & JSONL → docs/concepts/logging-jsonl.md
- Unsafe & RT ops → docs/concepts/unsafe-os.md
- Hardware abstraction → docs/concepts/hardware-abstraction.md
- Testing strategy → docs/testing/Strategy.md
- Build/CI & packaging → docs/concepts/build-ci.md

2. Architecture overview → docs/architecture/Overview.md
3. Modules index → docs/architecture/Modules.md
4. Data flow → docs/architecture/DataFlow.md
5. ADR: Predictive stop → docs/adr/ADR-001-predictive-stop.md
6. Ops runbook → docs/ops/Runbook.md
7. Glossary → docs/Glossary.md

Project tree with purpose:

- `doser_traits/` — traits `Scale`, `Motor`, `Clock` (see `src/lib.rs`, `src/clock.rs`).
- `doser_core/` — engine (builder, control loop, filters, predictor, runner) and tests.
- `doser_hardware/` — `hardware` feature for Pi GPIO; `sim` fallback; `pacing` utilities.
- `doser_config/` — TOML schema, validation, calibration CSV + robust refit.
- `doser_cli/` — CLI, JSONL output, tracing, RT helpers using libc.
- `docs/` — this handbook and deeper pages.

## B. Rust Primer for Java/.NET Developers

- Ownership/borrowing/lifetimes — we pass references or Box<dyn Trait> across crates; see `doser_traits::Scale` and `doser_core::DoserBuilder` setters. Lifetimes are implicit in trait bounds and channel threads; we avoid long-lived borrows.
  - See: docs/concepts/ownership-borrowing.md
- Traits vs interfaces — `Scale`/`Motor` are interfaces; we use trait objects (`Box<dyn Scale>`) and generics (`DoserCore<S,M>`). Comparable to Java interfaces with generics; trait objects ~ Java’s interface references with dynamic dispatch.
  - See: docs/concepts/traits-generics.md
- Results & error handling — Libraries use `thiserror` for typed errors (`doser_core::error::DoserError`, `doser_hardware::error::HwError`). CLI uses `eyre`/`color-eyre` for rich reports and JSON.
  - See: docs/concepts/error-handling.md
- Enums & pattern matching — `AbortReason` is an enum used with match to drive exit codes and JSON.
  - See: docs/concepts/enums-matching.md
- Concurrency & timing — `crossbeam_channel` for sampler; `MonotonicClock` abstracts timing; worker thread in hardware motor; rate control in `pacing::Pacer`.
  - See: docs/concepts/concurrency.md and docs/concepts/time.md
- Fixed-point math — `centigrams (cg)` + integer math inside `doser_core` for determinism. Median/MA/EMA filters are implemented over cg.
  - See: docs/concepts/fixed-point-and-filters.md

## C. Architecture

- Crates & boundaries:
  - `doser_traits`: contracts for hardware (`Scale`, `Motor`) and `Clock`.
  - `doser_core`: dosing pipeline, predictor, error types; no GPIO.
  - `doser_hardware`: Pi-only GPIO implementation and simulator.
  - `doser_cli`: orchestration and operator interface (JSONL/tracing/RT).
- Data flow: scale → filters → controller (bands/slowdown → predictive stop → settle) → motor.
  - See: docs/architecture/DataFlow.md
- Configuration: `doser_config::Config` from TOML, with validation; persisted calibration or CSV.

## D. Business Rules (with code pointers)

- Guards: `max_run_ms`, `max_overshoot_g`, `no_progress_ms/epsilon` → `doser_core/src/lib.rs` (DoserCore.step/step_from_raw).
- E-stop with debounce/latch → `DoserCore::poll_estop`, fields `estop_debounce_n`, `estop_latched`.
- Two-phase dosing: bands/slowdown controlled by `ControlCfg`; settle window via `stable_ms` and `epsilon_g`.
- Predictive stop: `maybe_early_stop` uses EMA-like slope and inflight mass.

## E. Observability

- `tracing` configured in CLI (`doser_cli/src/main.rs::init_tracing`).
- JSONL per-dose record produced in `doser_cli/src/main.rs` with stable keys: `timestamp,target_g,final_g,duration_ms,profile,slope_ema,stop_at_g,coast_comp_g,abort_reason`.

## F. Deployment & Ops

- See docs/ops/Runbook.md (systemd, logrotate, non-root, paths). `install.sh` provisions files.

## G. Testing Strategy

- Unit + property (`proptest`) in core; CLI integration tests; fuzz target for config TOML; benches via Criterion; coverage via tarpaulin.
- See docs/testing/Strategy.md

## H. Unsafe & OS Integration

- `doser_cli` uses libc for RT features on Linux/macOS: `mlockall`, `sched_setscheduler`, `sched_setaffinity`, CPU\_\* macros; all wrapped with safety notes and error handling.
- `doser_hardware` optionally elevates RT in the step thread when feature `rt` enabled.
- See docs/concepts/unsafe-os.md

## I. Gotchas & Patterns

- No unwrap/expect in non-test builds; errors bubble via `eyre` or typed errors.
- Fixed-point math inside core ensures deterministic thresholds; floats are for telemetry only.
- Sim backend exists; don’t couple core to GPIO.

## J. Glossary + FAQ

- See docs/glossary.md for Rust↔Java/C# mappings and domain terms.

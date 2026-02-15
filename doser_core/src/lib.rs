#![cfg_attr(all(not(debug_assertions), not(test)), deny(warnings))]
#![cfg_attr(
    all(not(debug_assertions), not(test)),
    deny(clippy::all, clippy::pedantic, clippy::nursery)
)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
//! Core dosing logic (hardware-agnostic).
//!
//! This crate provides the hardware-independent dosing engine. All hardware
//! interactions go through `doser_traits::Scale` and `doser_traits::Motor` traits.
//!
//! ## Architecture
//!
//! - **Calibration**: Linear model for raw→grams conversion (`calibration` module)
//! - **Configuration**: All config structs (`config` module)
//! - **Fixed-point**: Centigram arithmetic helpers (`fixed_point` module)
//! - **Filtering**: Median, moving average, EMA smoothing
//! - **Control**: Multi-speed control with hysteresis (`DoserCore`)
//! - **Safety**: Watchdogs for runtime, overshoot, no-progress
//! - **Status**: Dosing state machine (`status` module)
//! - **Builder**: Type-state builder pattern (`builder` module)
//!
//! ## Fixed-Point Arithmetic
//!
//! Internals operate in **centigrams** (cg, 1 cg = 0.01 g) using `i32` for deterministic
//! behavior. See `Calibration::to_cg` for conversion.

// ── Module declarations ──────────────────────────────────────────────────────

pub mod builder;
pub mod calibration;
pub mod config;
pub mod conversions;
mod core;
pub mod error;
pub mod fixed_point;
pub mod hw_error;
pub mod mocks;
pub mod runner;
pub mod sampler;
pub mod status;
pub mod util;

// ── Public re-exports (backward-compatible API) ──────────────────────────────

pub use builder::{Doser, DoserBuilder, DoserG, Missing, Set, build_doser};
pub use calibration::Calibration;
pub use config::{ControlCfg, FilterCfg, FilterKind, PredictorCfg, SafetyCfg, Timeouts};
pub use core::DoserCore;
pub use status::DosingStatus;

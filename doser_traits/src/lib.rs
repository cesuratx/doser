#![cfg_attr(all(not(debug_assertions), not(test)), deny(warnings))]
#![cfg_attr(
    all(not(debug_assertions), not(test)),
    deny(clippy::all, clippy::pedantic, clippy::nursery)
)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
//! Traits that define the hardware and time abstractions used by the system.
//!
//! - `Scale` provides a blocking `read(timeout)` API that returns a raw ADC
//!   reading in counts (i32). Calibration in `doser_core` converts counts to
//!   grams/centigrams. (The simulation backend happens to use a 1 count = 0.01 g
//!   scale, so its raw counts equal centigrams, but that is not part of the contract.)
//! - `Motor` configures/starts/stops motor stepping at steps-per-second.
//! - `clock` offers a `MonotonicClock` for deterministic timing and testability.
//!
//! Other crates depend only on these traits, enabling simulation and multiple hardware
//! backends while keeping `doser_core` hardware-agnostic.
pub mod clock;

pub use clock::{Clock, MonotonicClock};

pub trait Scale {
    /// Read one raw ADC sample in counts, blocking up to `timeout`.
    fn read(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>>;
}

pub trait Motor {
    fn set_speed(
        &mut self,
        steps_per_sec: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

// Allow boxed trait objects (Box<dyn Scale/Motor>) to be used where a generic S: Scale / M: Motor is expected.
impl<T: ?Sized + Scale> Scale for Box<T> {
    fn read(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        (**self).read(timeout)
    }
}

impl<T: ?Sized + Motor> Motor for Box<T> {
    fn set_speed(
        &mut self,
        steps_per_sec: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        (**self).set_speed(steps_per_sec)
    }
    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        (**self).stop()
    }
    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        (**self).start()
    }
}

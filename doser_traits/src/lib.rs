pub mod clock;

pub use clock::{Clock, MonotonicClock};

pub trait Scale {
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

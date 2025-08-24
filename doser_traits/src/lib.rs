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

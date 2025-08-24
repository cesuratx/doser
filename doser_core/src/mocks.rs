//! Test and helper mocks for doser_core

/// A scale that always errors on read; useful when driving the control loop
/// with externally sampled raw values via `step_from_raw`.
pub struct NoopScale;

impl doser_traits::Scale for NoopScale {
    fn read(
        &mut self,
        _timeout: std::time::Duration,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(std::io::Error::other("noop scale")))
    }
}

use std::time::Duration;

use crate::error::{HwError, Result};
use doser_traits::clock::Clock;

/// Wait until the provided `is_high` predicate becomes false (i.e., line goes low),
/// or a timeout expires. Sleeps in small intervals to avoid CPU spinning.
pub fn wait_until_low_with_timeout(
    mut is_high: impl FnMut() -> bool,
    timeout: Duration,
    poll_interval: Duration,
    clock: &dyn Clock,
) -> Result<()> {
    let start = clock.now();
    while is_high() {
        // Abort on timeout
        if clock.ms_since(start) >= timeout.as_millis() as u64 {
            return Err(HwError::DataReadyTimeout);
        }
        clock.sleep(poll_interval);
    }
    Ok(())
}

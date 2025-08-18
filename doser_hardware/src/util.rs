use std::time::{Duration, Instant};

use crate::error::{HwError, Result};

/// Wait until the provided `is_high` predicate becomes false (i.e., line goes low),
/// or a timeout expires. Sleeps in small intervals to avoid CPU spinning.
pub fn wait_until_low_with_timeout(
    mut is_high: impl FnMut() -> bool,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while is_high() {
        if Instant::now() >= deadline {
            return Err(HwError::DataReadyTimeout);
        }
        std::thread::sleep(poll_interval);
    }
    Ok(())
}

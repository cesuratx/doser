use std::time::Duration;

use crate::error::{HwError, Result};
use doser_traits::clock::Clock;

/// Busy-wait for at least ~1.5 microseconds to cleanly separate GPIO edges.
///
/// Used for HX711 SCK pulse timing (datasheet minimum high/low ~0.2µs) and motor
/// STEP edges. We measure elapsed time against a monotonic `Instant` rather than
/// spinning a calibrated iteration count: on the Pi 5's RP1 GPIO a spin-count
/// calibration proved marginal — `spin_loop()` is ~1 cycle on aarch64, so the
/// derived count produced sub-microsecond, jittery pulses that the HX711 missed,
/// returning garbage (0/-1) instead of a stable conversion. An `Instant` deadline
/// guarantees the minimum width on every edge. 1.5µs is comfortably above the
/// device minimum and well below the HX711's ~50µs power-down threshold.
#[inline]
pub fn busy_wait_min_1us() {
    use std::time::Instant;
    let start = Instant::now();
    let min = Duration::from_nanos(1_500);
    while start.elapsed() < min {
        std::hint::spin_loop();
    }
}

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

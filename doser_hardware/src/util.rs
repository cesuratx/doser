use std::time::Duration;

use crate::error::{HwError, Result};
use doser_traits::clock::Clock;

/// Busy-wait for at least ~1 microsecond to cleanly separate GPIO edges.
///
/// Used for HX711 SCK pulse timing (datasheet minimum high/low ~0.2µs) and motor
/// STEP edges. A single `spin_loop()` hint can be only a few nanoseconds — well
/// below the device minimum on a fast Pi — so we calibrate a rough spin count for
/// ~1µs once per process and spin that many times. 1µs is comfortably above the
/// minimum and well below the HX711's ~50µs power-down threshold.
#[inline]
pub fn busy_wait_min_1us() {
    #[inline]
    fn spins_per_us() -> u32 {
        use std::sync::OnceLock;
        use std::time::Instant;
        static SPINS: OnceLock<u32> = OnceLock::new();
        *SPINS.get_or_init(|| {
            // Increase iteration counts until we measure ≥ 100µs to reduce timer noise.
            let mut iters: u32 = 1_000;
            let mut per_us: u32 = 2; // conservative fallback
            for _ in 0..10 {
                let start = Instant::now();
                let mut i = 0u32;
                while i < iters {
                    std::hint::spin_loop();
                    i = i.wrapping_add(1);
                }
                let dt = start.elapsed();
                if dt >= Duration::from_micros(100) {
                    let us = u64::try_from(dt.as_micros().max(1)).unwrap_or(u64::MAX);
                    per_us = u32::try_from(u64::from(iters).div_ceil(us))
                        .unwrap_or(u32::MAX)
                        .clamp(1, 1_000_000);
                    break;
                }
                iters = iters.saturating_mul(4).min(4_000_000);
            }
            per_us.max(1)
        })
    }

    let n = spins_per_us();
    let mut i = 0u32;
    while i < n {
        std::hint::spin_loop();
        i = i.wrapping_add(1);
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

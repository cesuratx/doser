use std::thread;
use std::time::{Duration, Instant};

/// Monotonic clock abstraction for control and timing across the stack.
///
/// - `now()`: returns a monotonic `Instant`
/// - `sleep()`: sleeps for the provided duration (implementations may simulate)
/// - `ms_since()`: helper to compute elapsed milliseconds from an epoch `Instant`
pub trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, d: Duration);

    /// Milliseconds elapsed since `epoch`, saturating at 0 on underflow and at
    /// `u64::MAX` on overflow (~584 million years, so unreachable in practice).
    fn ms_since(&self, epoch: Instant) -> u64 {
        let dur = self.now().saturating_duration_since(epoch);
        duration_to_ms(dur)
    }
}

/// Whole milliseconds in `dur`, saturating at `u64::MAX`.
///
/// `Duration::as_millis` yields a `u128`; the saturating conversion keeps the
/// house-wide clamping behaviour without a truncating `as` cast. The clamp is
/// unreachable for any realistic elapsed time.
#[inline]
#[must_use]
pub fn duration_to_ms(dur: Duration) -> u64 {
    u64::try_from(dur.as_millis()).unwrap_or(u64::MAX)
}

/// Default, real-time monotonic clock backed by `std::time::Instant`.
#[derive(Debug, Default, Clone, Copy)]
pub struct MonotonicClock;

impl MonotonicClock {
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Clock for MonotonicClock {
    #[inline]
    fn now(&self) -> Instant {
        Instant::now()
    }

    #[inline]
    fn sleep(&self, d: Duration) {
        if d.is_zero() {
            return;
        }
        thread::sleep(d);
    }
}

#[cfg(test)]
pub mod test_clock {
    use super::*;

    /// Deterministic test clock whose time can be advanced manually.
    ///
    /// now() = origin + offset
    /// sleep(d) advances internal time by d without actually sleeping.
    #[derive(Debug, Clone)]
    pub struct TestClock {
        origin: Instant,
        offset: std::sync::Arc<std::sync::Mutex<Duration>>,
    }

    impl Default for TestClock {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestClock {
        pub fn new() -> Self {
            Self {
                origin: Instant::now(),
                offset: std::sync::Arc::new(std::sync::Mutex::new(Duration::ZERO)),
            }
        }

        /// Advance the clock by the given duration.
        pub fn advance(&self, d: Duration) {
            if let Ok(mut off) = self.offset.lock() {
                *off = off.saturating_add(d);
            }
        }

        /// Set the absolute offset relative to origin (useful for tests).
        pub fn set_offset(&self, d: Duration) {
            if let Ok(mut off) = self.offset.lock() {
                *off = d;
            }
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            let off = self.offset.lock().map(|g| *g).unwrap_or(Duration::ZERO);
            self.origin + off
        }

        fn sleep(&self, d: Duration) {
            self.advance(d);
        }
    }
}

//! Background sensor sampling utilities.
//!
//! Spawns a thread that owns the `Scale`, pushes latest readings via a
//! bounded channel, and tracks the last-ok timestamp for watchdog logic.
//! Event-driven and paced variants are provided.
//!
//! Safety: Each `Sampler` spawns exactly one thread that is automatically
//! shut down when the `Sampler` is dropped, preventing thread leaks.
use crossbeam_channel as xch;
use doser_traits::Scale;
use doser_traits::clock::Clock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct Sampler {
    rx: xch::Receiver<i32>,
    last_ok: Arc<AtomicU64>,
    epoch: Instant,
    /// Shutdown flag for immediate response (atomic for lock-free check)
    shutdown: Arc<AtomicBool>,
    /// Join handle for graceful thread cleanup
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl Sampler {
    pub fn spawn<S: Scale + Send + 'static, C: Clock + Send + Sync + 'static>(
        mut scale: S,
        hz: u32,
        timeout: Duration,
        clock: C,
    ) -> Self {
        let (tx, rx) = xch::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let last_ok = Arc::new(AtomicU64::new(0));
        let last_ok_clone = last_ok.clone();
        let period = Duration::from_micros(crate::util::period_us(hz));
        let epoch = clock.now();

        let join_handle = std::thread::spawn(move || {
            loop {
                // Immediate shutdown check (lock-free atomic)
                if shutdown_clone.load(Ordering::Relaxed) {
                    tracing::debug!("Sampler thread received shutdown signal");
                    break;
                }

                match scale.read(timeout) {
                    Ok(v) => {
                        // If send fails, consumer is gone; exit gracefully
                        if tx.send(v).is_err() {
                            tracing::debug!("Sampler consumer disconnected, exiting thread");
                            break;
                        }
                        let now = clock.ms_since(epoch);
                        last_ok_clone.store(now, Ordering::Relaxed);
                    }
                    Err(_) => {
                        // Optional: send special value or skip; controller has watchdog
                    }
                }

                // Check shutdown before sleep to avoid unnecessary delay
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                clock.sleep(period);
            }
            tracing::trace!("Sampler thread exiting cleanly");
        });

        Self {
            rx,
            last_ok,
            epoch,
            shutdown,
            join_handle: Some(join_handle),
        }
    }

    /// Event-driven sampler: rely on the sensor's own data-ready timing and do not add extra sleeps.
    /// The scale.read(timeout) should block until data is ready or timeout expires.
    pub fn spawn_event<S: Scale + Send + 'static, C: Clock + Send + Sync + 'static>(
        mut scale: S,
        timeout: Duration,
        clock: C,
    ) -> Self {
        let (tx, rx) = xch::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let last_ok = Arc::new(AtomicU64::new(0));
        let last_ok_clone = last_ok.clone();
        let epoch = clock.now();

        let join_handle = std::thread::spawn(move || {
            loop {
                // Immediate shutdown check (lock-free atomic)
                if shutdown_clone.load(Ordering::Relaxed) {
                    tracing::debug!("Sampler event thread received shutdown signal");
                    break;
                }

                match scale.read(timeout) {
                    Ok(v) => {
                        // If send fails, consumer is gone; exit gracefully
                        if tx.send(v).is_err() {
                            tracing::debug!("Sampler event consumer disconnected, exiting thread");
                            break;
                        }
                        let now = clock.ms_since(epoch);
                        last_ok_clone.store(now, Ordering::Relaxed);
                    }
                    Err(_) => {
                        // On timeout or transient error, just continue; controller will watchdog
                    }
                }
                // No sleep here: next iteration will block in read() until DRDY
                // But check shutdown immediately after read completes
            }
            tracing::trace!("Sampler event thread exiting cleanly");
        });

        Self {
            rx,
            last_ok,
            epoch,
            shutdown,
            join_handle: Some(join_handle),
        }
    }

    pub fn latest(&self) -> Option<i32> {
        self.rx.try_iter().last()
    }
    pub fn stalled_for(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_ok.load(Ordering::Relaxed))
    }
    /// Convenience helper: compute stall using this sampler's epoch and a real monotonic clock.
    pub fn stalled_for_now(&self) -> u64 {
        let now_ms = {
            let dur = Instant::now().saturating_duration_since(self.epoch);
            let ms = dur.as_millis();
            (ms.min(u128::from(u64::MAX))) as u64
        };
        now_ms.saturating_sub(self.last_ok.load(Ordering::Relaxed))
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        // Signal shutdown immediately (atomic store is very fast, <10ns)
        self.shutdown.store(true, Ordering::Relaxed);

        // For dosing systems, we need prompt response but must handle blocking reads.
        // The thread will exit:
        // 1. Immediately if it's between reads (checking shutdown flag)
        // 2. After current scale.read() completes (up to sensor timeout, ~150ms worst case)
        // 3. Immediately after read if it was in sleep (shutdown check added before sleep)
        if let Some(handle) = self.join_handle.take() {
            match handle.join() {
                Ok(()) => {
                    tracing::trace!("Sampler thread joined successfully");
                }
                Err(e) => {
                    // Thread panicked; log but don't propagate (we're in Drop)
                    tracing::warn!(?e, "Sampler thread panicked during shutdown");
                }
            }
        }
    }
}

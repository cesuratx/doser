use crossbeam_channel as xch;
use doser_traits::Scale;
use doser_traits::clock::Clock;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct Sampler {
    rx: xch::Receiver<i32>,
    last_ok: Arc<AtomicU64>,
    epoch: Instant,
}

impl Sampler {
    pub fn spawn<S: Scale + Send + 'static, C: Clock + Send + Sync + 'static>(
        mut scale: S,
        hz: u32,
        timeout: Duration,
        clock: C,
    ) -> Self {
        let (tx, rx) = xch::bounded(1);
        let last_ok = Arc::new(AtomicU64::new(0));
        let last_ok_clone = last_ok.clone();
        let period = Duration::from_micros(1_000_000u64 / hz as u64);
        let epoch = clock.now();

        std::thread::spawn(move || {
            loop {
                match scale.read(timeout) {
                    Ok(v) => {
                        let _ = tx.send(v);
                        let now = clock.ms_since(epoch);
                        last_ok_clone.store(now, Ordering::Relaxed);
                    }
                    Err(_) => {
                        // Optional: send special value or skip; controller has watchdog
                    }
                }
                clock.sleep(period);
            }
        });

        Self { rx, last_ok, epoch }
    }

    pub fn latest(&self) -> Option<i32> {
        self.rx.try_iter().last()
    }
    pub fn stalled_for(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_ok.load(Ordering::Relaxed))
    }
    /// Convenience helper: compute stall using this sampler's epoch and a real monotonic clock.
    pub fn stalled_for_now(&self) -> u64 {
        let now_ms = Instant::now()
            .saturating_duration_since(self.epoch)
            .as_millis() as u64;
        now_ms.saturating_sub(self.last_ok.load(Ordering::Relaxed))
    }
}

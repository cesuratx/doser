use crossbeam_channel as xch;
use doser_traits::Scale;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct Sampler {
    rx: xch::Receiver<i32>,
    last_ok: Arc<AtomicU64>,
}

impl Sampler {
    pub fn spawn<S: Scale + Send + 'static>(mut scale: S, hz: u32, timeout: Duration) -> Self {
        let (tx, rx) = xch::bounded(1);
        let last_ok = Arc::new(AtomicU64::new(now_ms()));
        let last_ok_clone = last_ok.clone();

        std::thread::spawn(move || {
            let period = Duration::from_micros((1_000_000u64 / hz as u64) as u64);
            loop {
                let t0 = Instant::now();
                match scale.read(timeout) {
                    Ok(v) => {
                        let _ = tx.send(v);
                        last_ok_clone.store(now_ms(), Ordering::Relaxed);
                    }
                    Err(_) => {
                        // Optional: send special value or skip; controller has watchdog
                    }
                }
                let elapsed = t0.elapsed();
                if elapsed < period {
                    std::thread::sleep(period - elapsed);
                }
            }
        });

        Self { rx, last_ok }
    }

    pub fn latest(&self) -> Option<i32> {
        self.rx.try_iter().last()
    }
    pub fn stalled_for(&self, now_ms: u64) -> u64 {
        now_ms - self.last_ok.load(Ordering::Relaxed)
    }
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

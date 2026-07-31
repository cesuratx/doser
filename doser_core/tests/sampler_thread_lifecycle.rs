//! Sampler thread lifecycle and cleanup.
//!
//! Every test here used to be structurally incapable of failing: they slept,
//! called `drop`, and asserted nothing — "test passes if no panic occurs" is not
//! a leak detector, because `Sampler::drop` joins the thread, so a leak shows up
//! as a *hang*, not as a panic. Two changes fix that:
//!
//! 1. `TrackedScale` makes thread liveness observable. The spawned closure owns
//!    the `Scale`, so the shared counter returns to zero exactly when that
//!    thread's stack unwinds. A thread that returned from `join` without having
//!    dropped its scale — or a `Sampler` that stopped joining at all — is caught.
//! 2. Every `drop` runs on a worker thread behind `recv_timeout`, so a thread
//!    that never exits fails the test instead of wedging the whole suite.
//!
//! The old `sampler_shutdown_is_prompt` asserted a 200 ms wall-clock bound while
//! the sampler legitimately sleeps a full sampling period (100 ms at its hz=10)
//! that `Drop` cannot interrupt — that assertion measured the CI runner's
//! scheduler, not this code. It is replaced below with a bound derived from the
//! sampler's own documented worst case.

use doser_core::sampler::Sampler;
use doser_traits::Scale;
use doser_traits::clock::MonotonicClock;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// ── Fixtures ─────────────────────────────────────────────────────────────────

/// What a `TrackedScale::read` should do.
#[derive(Clone, Copy)]
enum ReadBehaviour {
    /// Fail immediately, like a sensor with no data ready.
    Fail,
    /// Succeed immediately, so the bounded channel fills when nobody drains it.
    Ok(i32),
    /// Succeed after a fixed delay, modelling a blocking DRDY wait.
    OkAfter(i32, Duration),
}

/// Scale that reports whether the thread owning it is still alive.
///
/// `alive` is incremented on construction and decremented in `Drop`. The sampler
/// moves the scale into its thread, so `alive == 0` after a join proves the
/// thread actually finished rather than merely being unreachable.
struct TrackedScale {
    alive: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
    behaviour: ReadBehaviour,
}

impl TrackedScale {
    fn new(alive: &Arc<AtomicUsize>, behaviour: ReadBehaviour) -> (Self, Arc<AtomicUsize>) {
        alive.fetch_add(1, Ordering::SeqCst);
        let reads = Arc::new(AtomicUsize::new(0));
        (
            Self {
                alive: alive.clone(),
                reads: reads.clone(),
                behaviour,
            },
            reads,
        )
    }
}

impl Drop for TrackedScale {
    fn drop(&mut self) {
        self.alive.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Scale for TrackedScale {
    fn read(
        &mut self,
        _timeout: Duration,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        match self.behaviour {
            ReadBehaviour::Fail => Err(Box::new(std::io::Error::other("no data ready"))),
            ReadBehaviour::Ok(v) => Ok(v),
            ReadBehaviour::OkAfter(v, d) => {
                std::thread::sleep(d);
                Ok(v)
            }
        }
    }
}

/// Drop the sampler on a worker thread so a thread that never exits fails this
/// test rather than hanging the suite. Returns how long the drop took; most
/// callers only care that it returned at all.
fn drop_within(sampler: Sampler, budget: Duration) -> Duration {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let start = Instant::now();
        drop(sampler);
        let _ = tx.send(start.elapsed());
    });
    let elapsed = rx.recv_timeout(budget).unwrap_or_else(|_| {
        // The worker is still blocked in `Sampler::drop`; joining it here would
        // hang too, so leave it and fail loudly.
        panic!("Sampler::drop did not complete within {budget:?}: the sampler thread leaked");
    });
    let _ = handle.join();
    elapsed
}

/// Spin until `reads` reaches `n`, so the assertions below run against a thread
/// that has provably started. This is a bounded wait, not a timing assertion.
fn wait_for_reads(reads: &AtomicUsize, n: usize, budget: Duration) {
    let deadline = Instant::now() + budget;
    while reads.load(Ordering::SeqCst) < n {
        assert!(
            Instant::now() < deadline,
            "sampler thread performed {} of {n} expected reads within {budget:?}",
            reads.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

const LEAK_BUDGET: Duration = Duration::from_secs(5);

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn paced_sampler_thread_releases_its_scale_on_drop() {
    let alive = Arc::new(AtomicUsize::new(0));
    let (scale, reads) = TrackedScale::new(&alive, ReadBehaviour::Fail);
    let sampler = Sampler::spawn(
        scale,
        100,
        Duration::from_millis(100),
        MonotonicClock::new(),
    );

    wait_for_reads(&reads, 1, Duration::from_secs(5));
    assert_eq!(alive.load(Ordering::SeqCst), 1, "the thread should be live");

    drop_within(sampler, LEAK_BUDGET);
    assert_eq!(
        alive.load(Ordering::SeqCst),
        0,
        "the sampler thread returned from join without dropping its Scale"
    );
}

#[test]
fn event_sampler_thread_releases_its_scale_on_drop() {
    let alive = Arc::new(AtomicUsize::new(0));
    let (scale, reads) = TrackedScale::new(&alive, ReadBehaviour::Fail);
    let sampler = Sampler::spawn_event(scale, Duration::from_millis(100), MonotonicClock::new());

    wait_for_reads(&reads, 1, Duration::from_secs(5));
    assert_eq!(alive.load(Ordering::SeqCst), 1);

    drop_within(sampler, LEAK_BUDGET);
    assert_eq!(
        alive.load(Ordering::SeqCst),
        0,
        "the event sampler thread returned from join without dropping its Scale"
    );
}

#[test]
fn multiple_samplers_dont_leak_threads() {
    // One shared counter across all iterations: if any sampler's thread outlives
    // its `drop`, the count never returns to zero and every later assertion
    // fails too, so the leak cannot be masked by a subsequent clean iteration.
    let alive = Arc::new(AtomicUsize::new(0));

    for i in 0..10 {
        let (scale, reads) = TrackedScale::new(&alive, ReadBehaviour::Fail);
        let sampler = Sampler::spawn(scale, 200, Duration::from_millis(50), MonotonicClock::new());
        wait_for_reads(&reads, 1, Duration::from_secs(5));
        assert_eq!(alive.load(Ordering::SeqCst), 1, "iteration {i}");

        // Draining is part of normal use; it must not affect shutdown.
        let _ = sampler.latest();
        drop_within(sampler, LEAK_BUDGET);
        assert_eq!(
            alive.load(Ordering::SeqCst),
            0,
            "iteration {i} leaked a sampler thread"
        );
    }
}

#[test]
fn sampler_can_be_created_dropped_and_recreated() {
    let alive = Arc::new(AtomicUsize::new(0));

    for i in 0..3 {
        let (scale, reads) = TrackedScale::new(&alive, ReadBehaviour::Fail);
        let sampler = Sampler::spawn(scale, 200, Duration::from_millis(50), MonotonicClock::new());
        wait_for_reads(&reads, 1, Duration::from_secs(5));
        drop_within(sampler, LEAK_BUDGET);
        assert_eq!(alive.load(Ordering::SeqCst), 0, "sampler {i} leaked");
    }
}

#[test]
fn sampler_thread_exits_when_the_consumer_never_drains_the_channel() {
    // The producer's `try_send` has a `Disconnected` arm, but `Sampler` owns both
    // ends of the channel and its `Drop` signals and joins *before* the receiver
    // field is dropped, so that arm is unreachable in practice. What is reachable
    // — and what actually threatened a deadlock — is a consumer that stops
    // draining: the bounded(1) channel stays full, and a blocking send would wedge
    // the join forever. This is the case that must be covered.
    let alive = Arc::new(AtomicUsize::new(0));
    let (scale, reads) = TrackedScale::new(&alive, ReadBehaviour::Ok(42));
    let sampler = Sampler::spawn(
        scale,
        1_000,
        Duration::from_millis(10),
        MonotonicClock::new(),
    );

    // Several successful reads with nothing consumed: the channel is now full.
    wait_for_reads(&reads, 5, Duration::from_secs(5));

    drop_within(sampler, LEAK_BUDGET);
    assert_eq!(
        alive.load(Ordering::SeqCst),
        0,
        "the sampler thread did not exit while the channel was full"
    );
}

#[test]
fn sampler_drop_is_prompt_with_full_channel() {
    // Same shape as above, kept as a distinct regression: the original bug was a
    // *blocking* send deadlocking `Drop`'s join. The budget here is a deadlock
    // detector, not a latency measurement.
    let alive = Arc::new(AtomicUsize::new(0));
    let (scale, reads) = TrackedScale::new(&alive, ReadBehaviour::Ok(42));
    let sampler = Sampler::spawn(
        scale,
        1_000,
        Duration::from_millis(10),
        MonotonicClock::new(),
    );
    wait_for_reads(&reads, 5, Duration::from_secs(5));

    drop_within(sampler, Duration::from_secs(3));
    assert_eq!(alive.load(Ordering::SeqCst), 0);
}

#[test]
fn sampler_shutdown_does_not_wait_for_the_sensor_timeout() {
    // For a dosing system shutdown must be bounded, but the bound has to be one
    // the code actually promises. The producer loop is
    //   check shutdown -> scale.read(timeout) -> check shutdown -> clock.sleep(period)
    // and neither the read nor the sleep is interruptible, so the honest worst
    // case is `read latency + one period + join`. Here that is 20 + 20 ms, while
    // the per-read *timeout* is 3 s.
    //
    // A 500 ms budget is 12x the real worst case — loose enough that scheduler
    // noise on a loaded runner cannot trip it, tight enough that a regression
    // which made `Drop` wait out the sensor timeout (3 s) fails immediately.
    const READ_MS: u64 = 20;
    const PERIOD_HZ: u32 = 50; // 20 ms period

    let alive = Arc::new(AtomicUsize::new(0));
    let (scale, reads) = TrackedScale::new(
        &alive,
        ReadBehaviour::OkAfter(7, Duration::from_millis(READ_MS)),
    );
    let sampler = Sampler::spawn(
        scale,
        PERIOD_HZ,
        Duration::from_secs(3),
        MonotonicClock::new(),
    );
    wait_for_reads(&reads, 2, Duration::from_secs(5));

    let elapsed = drop_within(sampler, Duration::from_millis(500));
    assert_eq!(
        alive.load(Ordering::SeqCst),
        0,
        "shutdown completed without the thread exiting"
    );
    // Recorded rather than asserted tightly: useful signal in CI logs without
    // turning scheduler jitter into a failure.
    if elapsed > Duration::from_millis(200) {
        eprintln!("note: sampler shutdown took {elapsed:?} (worst case is ~40 ms + join)");
    }
}

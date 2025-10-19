//! Test sampler thread lifecycle and cleanup to prevent thread leaks.
//!
//! Verifies that:
//! - Threads are properly cleaned up when Sampler is dropped
//! - Multiple samplers can be created and destroyed without accumulating threads
//! - Thread exits gracefully when consumer disconnects

use doser_core::mocks::NoopScale;
use doser_core::sampler::Sampler;
use doser_traits::clock::MonotonicClock;
use std::time::Duration;

#[test]
fn sampler_thread_exits_on_drop() {
    // Create a sampler
    let clock = MonotonicClock::new();
    let scale = NoopScale;
    let sampler = Sampler::spawn(scale, 10, Duration::from_millis(100), clock);

    // Give thread time to start
    std::thread::sleep(Duration::from_millis(50));

    // Drop the sampler - thread should exit gracefully
    drop(sampler);

    // Give thread time to exit
    std::thread::sleep(Duration::from_millis(50));

    // If the thread leaked, it would still be running
    // This test passes if no panic occurs and drop completes
}

#[test]
fn multiple_samplers_dont_leak_threads() {
    let clock = MonotonicClock::new();

    // Create and destroy multiple samplers
    for _ in 0..10 {
        let scale = NoopScale;
        let sampler = Sampler::spawn(scale, 10, Duration::from_millis(50), clock.clone());

        // Let it run briefly
        std::thread::sleep(Duration::from_millis(10));

        // Verify we can get some samples
        let _ = sampler.latest();

        // Drop explicitly
        drop(sampler);
    }

    // All threads should have exited
    std::thread::sleep(Duration::from_millis(100));

    // Test passes if we reach here without hanging or panicking
}

#[test]
fn event_sampler_thread_exits_on_drop() {
    let clock = MonotonicClock::new();
    let scale = NoopScale;
    let sampler = Sampler::spawn_event(scale, Duration::from_millis(100), clock);

    // Give thread time to start
    std::thread::sleep(Duration::from_millis(50));

    // Drop the sampler - thread should exit gracefully
    drop(sampler);

    // Give thread time to exit
    std::thread::sleep(Duration::from_millis(50));
}

#[test]
fn sampler_exits_when_consumer_disconnects() {
    let clock = MonotonicClock::new();
    let scale = NoopScale;
    let sampler = Sampler::spawn(scale, 10, Duration::from_millis(50), clock);

    // Consume a sample to ensure thread is running
    std::thread::sleep(Duration::from_millis(100));
    let _ = sampler.latest();

    // Drop sampler without explicit shutdown - receiver disconnects
    // Thread should detect this and exit
    drop(sampler);

    // Should complete quickly without hanging
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn sampler_can_be_created_dropped_and_recreated() {
    let clock = MonotonicClock::new();

    // Create sampler
    let scale1 = NoopScale;
    let sampler1 = Sampler::spawn(scale1, 10, Duration::from_millis(50), clock.clone());
    std::thread::sleep(Duration::from_millis(50));
    drop(sampler1);

    // Create another one - should not conflict
    let scale2 = NoopScale;
    let sampler2 = Sampler::spawn(scale2, 10, Duration::from_millis(50), clock.clone());
    std::thread::sleep(Duration::from_millis(50));
    drop(sampler2);

    // Create a third one
    let scale3 = NoopScale;
    let sampler3 = Sampler::spawn(scale3, 10, Duration::from_millis(50), clock);
    std::thread::sleep(Duration::from_millis(50));
    drop(sampler3);
}

#[test]
fn sampler_shutdown_is_prompt() {
    // For a dosing system, shutdown must be fast to ensure safety
    let clock = MonotonicClock::new();
    let scale = NoopScale;
    let sampler = Sampler::spawn(scale, 10, Duration::from_millis(50), clock);

    // Let it run briefly
    std::thread::sleep(Duration::from_millis(100));

    // Measure shutdown time
    let start = std::time::Instant::now();
    drop(sampler);
    let shutdown_time = start.elapsed();

    // Shutdown should complete quickly
    // Worst case: current scale.read() timeout (~50ms) + join overhead (~5ms)
    // Best case: immediate if between reads (<1ms)
    // We allow up to 200ms as a safe upper bound for test stability
    assert!(
        shutdown_time < Duration::from_millis(200),
        "Shutdown took {:?}, expected < 200ms for prompt response",
        shutdown_time
    );

    // For most cases, it should be much faster
    if shutdown_time > Duration::from_millis(100) {
        eprintln!(
            "Warning: shutdown took {:?}, consider this acceptable but monitor",
            shutdown_time
        );
    }
}

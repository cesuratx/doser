use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use doser_hardware::error::HwError;
use doser_hardware::util::wait_until_low_with_timeout;
use doser_traits::clock::MonotonicClock;
use rstest::rstest;

#[rstest]
fn wait_until_low_success_path() {
    let high = Arc::new(AtomicBool::new(true));
    let high_bg = high.clone();
    // Use a real clock here; this test just verifies behavior.
    let clock = MonotonicClock::new();
    // Flip low after a short delay in a real thread
    thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3));
        high_bg.store(false, Ordering::Relaxed);
    });

    let res = wait_until_low_with_timeout(
        || high.load(Ordering::Relaxed),
        Duration::from_millis(50),
        Duration::from_micros(200),
        &clock,
    );
    assert!(res.is_ok(), "expected success, got {res:?}");
}

#[rstest]
fn wait_until_low_timeout_path() {
    let high = Arc::new(AtomicBool::new(true));
    let clock = MonotonicClock::new();

    let err = wait_until_low_with_timeout(
        || high.load(Ordering::Relaxed),
        Duration::from_millis(5),
        Duration::from_micros(200),
        &clock,
    )
    .expect_err("expected timeout error");

    match err {
        HwError::DataReadyTimeout => {}
        other => panic!("unexpected error: {other:?}"),
    }
}

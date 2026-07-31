//! Tests for the two `apply_filter` branches that had no coverage at all.
//!
//! Every other test in the suite sets `ma_window: 1` or uses the EMA, so the
//! moving-average branch — its warm-up on a partially filled window, its
//! round-half-away-from-zero, and its i128 overflow fallback — never executed.
//! The median was only ever exercised with an odd window, leaving the even-window
//! path untested; that path is the trickiest code in the filter, because
//! `select_nth_unstable` leaves the lower partition *unsorted*, so the
//! lower-middle order statistic has to be recovered as `max(lower partition)`.
//!
//! All expected values below are hand-computed and stated in centigrams. The
//! calibration gain of 0.01 g/count makes one raw count exactly one centigram,
//! so the raw values fed in are also the filter's inputs.

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doser_core::{Calibration, ControlCfg, Doser, FilterCfg, SafetyCfg, Timeouts};
use doser_traits::clock::Clock;
use rstest::rstest;

#[derive(Default)]
struct NoopMotor;
impl doser_traits::Motor for NoopMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, _sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

/// Deterministic clock: `sleep` advances a virtual offset instead of blocking.
#[derive(Clone)]
struct ManualClock {
    origin: Instant,
    offset: Arc<Mutex<Duration>>,
}
impl ManualClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            offset: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }
}
impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.origin + *self.offset.lock().unwrap()
    }
    fn sleep(&self, d: Duration) {
        *self.offset.lock().unwrap() += d;
    }
}

/// A doser used purely as a filter harness: samples arrive via `step_from_raw`,
/// the target is far away so no control decision interferes, and every watchdog
/// is disabled or unreachable.
fn harness(filter: FilterCfg) -> Doser {
    Doser::builder()
        .with_scale(doser_core::mocks::NoopScale)
        .with_motor(NoopMotor)
        .with_filter(filter)
        .with_control(ControlCfg {
            speed_bands: vec![],
            stable_ms: 0,
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 600_000,
            max_overshoot_g: 5_000.0,
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_calibration(Calibration {
            gain_g_per_count: 0.01,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(100.0)
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"))
}

/// Feed one raw count and return the filtered weight in centigrams.
fn feed(doser: &mut Doser, raw: i32) -> i32 {
    doser
        .step_from_raw(raw)
        .unwrap_or_else(|e| panic!("step_from_raw({raw}): {e}"));
    (doser.last_weight() * 100.0).round() as i32
}

/// Feed a whole sequence and collect the filtered output for each sample.
fn feed_all(doser: &mut Doser, raws: &[i32]) -> Vec<i32> {
    raws.iter().map(|&r| feed(doser, r)).collect()
}

fn ma_filter(ma_window: usize) -> FilterCfg {
    FilterCfg {
        ma_window,
        median_window: 1,
        sample_rate_hz: 50,
        ema_alpha: 0.0,
    }
}

fn median_filter(median_window: usize) -> FilterCfg {
    FilterCfg {
        ma_window: 1,
        median_window,
        sample_rate_hz: 50,
        ema_alpha: 0.0,
    }
}

// ── Moving average ───────────────────────────────────────────────────────────

#[rstest]
fn moving_average_warms_up_over_a_partially_filled_window() {
    // The mean is taken over `ma_buf.len()`, not over `ma_window`, so the first
    // window-1 samples are averaged over a shorter buffer:
    //   10        -> 10/1 = 10
    //   10,20     -> 30/2 = 15
    //   10,20,30  -> 60/3 = 20
    //   10..40    -> 100/4 = 25
    //   20..50    -> 140/4 = 35   (the 10 has been evicted)
    let mut d = harness(ma_filter(4));
    assert_eq!(
        feed_all(&mut d, &[10, 20, 30, 40, 50]),
        vec![10, 15, 20, 25, 35]
    );
}

#[rstest]
fn moving_average_rounds_halves_away_from_zero() {
    // 1+2+3+4 = 10 over 4 samples is exactly 2.5, which must round to 3.
    let mut d = harness(ma_filter(4));
    assert_eq!(feed_all(&mut d, &[1, 2, 3, 4]).last().copied(), Some(3));

    // The mirrored negative case is -2.5, which must round to -3, not -2:
    // truncating division would bias every negative average toward zero.
    let mut d = harness(ma_filter(4));
    assert_eq!(
        feed_all(&mut d, &[-1, -2, -3, -4]).last().copied(),
        Some(-3)
    );
}

#[rstest]
fn moving_average_falls_back_to_i128_when_the_window_sum_overflows_i32() {
    // A single i32::MAX sample keeps the running sum inside i32, so the normal
    // `div_round_nearest_i32` path runs and the value passes through unchanged.
    let mut d = harness(ma_filter(2));
    d.step_from_raw(i32::MAX).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        d.last_weight(),
        (i32::MAX as f32) / 100.0,
        "a single-sample window must pass the value through"
    );

    // Adding 1 pushes the sum to 2^31, one past i32::MAX, which takes the i128
    // fallback: (2147483648 + 1) / 2 == 1073741824, still inside i32.
    d.step_from_raw(1).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        d.last_weight(),
        (1_073_741_824_i32 as f32) / 100.0,
        "the overflow fallback must produce the true mean, not a wrapped value"
    );
}

#[rstest]
fn ema_takes_precedence_over_the_moving_average_window() {
    // `apply_filter` checks `ema_alpha > 0.0` before `ma_window > 1`. With
    // alpha = 1.0 the EMA is the identity, so a configuration that sets both
    // must reproduce the input exactly rather than the moving average
    // (10, 15, 20, 25).
    let mut d = harness(FilterCfg {
        ma_window: 4,
        median_window: 1,
        sample_rate_hz: 50,
        ema_alpha: 1.0,
    });
    assert_eq!(
        feed_all(&mut d, &[10, 20, 30, 40]),
        vec![10, 20, 30, 40],
        "the moving-average window must be bypassed when the EMA is enabled"
    );
}

// ── Median, even window ──────────────────────────────────────────────────────

#[rstest]
fn median_even_window_averages_the_two_middle_order_statistics() {
    // window = 4, so the buffer is even-sized from the 2nd sample on except at
    // n = 1 and n = 3:
    //   [10]            n=1 odd  -> 10
    //   [10,20]         n=2 even -> mean(10,20)          = 15
    //   [10,20,31]      n=3 odd  -> 20
    //   [10,20,31,40]   n=4 even -> mean(20,31) = 25.5   -> 26 (away from zero)
    //   [20,31,40,100]  n=4 even -> mean(31,40) = 35.5   -> 36
    let mut d = harness(median_filter(4));
    assert_eq!(
        feed_all(&mut d, &[10, 20, 31, 40, 100]),
        vec![10, 15, 20, 26, 36]
    );
}

#[rstest]
fn median_even_window_is_independent_of_arrival_order() {
    // Same multiset as above in a different order. `select_nth_unstable(mid)`
    // only guarantees that index `mid` holds the mid-th order statistic and that
    // everything below it is <= it — the lower partition itself is *unsorted*.
    // Taking `lo[0]` or `lo.last()` instead of `lo.max()` would give 10 here and
    // yield 21 instead of 26, so this case pins that specific line.
    let mut d = harness(median_filter(4));
    assert_eq!(
        feed_all(&mut d, &[40, 10, 31, 20]).last().copied(),
        Some(26)
    );
}

#[rstest]
fn median_even_window_rounds_negative_halves_away_from_zero() {
    // Middles are -31 and -20; their mean is -25.5, which must round to -26.
    let mut d = harness(median_filter(4));
    assert_eq!(
        feed_all(&mut d, &[-40, -31, -20, -10]).last().copied(),
        Some(-26)
    );
}

#[rstest]
fn median_window_of_two_averages_every_adjacent_pair() {
    // The smallest even window: n is 1 then 2 forever.
    //   [10]      -> 10
    //   [10,21]   -> mean = 15.5 -> 16
    //   [21,20]   -> mean = 20.5 -> 21
    let mut d = harness(median_filter(2));
    assert_eq!(feed_all(&mut d, &[10, 21, 20]), vec![10, 16, 21]);
}

#[rstest]
fn median_prefilter_feeds_the_moving_average_stage() {
    // Both stages enabled: the median runs first, the moving average smooths its
    // output. With a 3-wide median and a 2-wide mean:
    //   raw 10 -> med [10]        = 10 -> ma [10]     = 10
    //   raw 20 -> med [10,20]     = 15 -> ma [10,15]  = 13   (25/2 = 12.5 -> 13)
    //   raw 900-> med [10,20,900] = 20 -> ma [15,20]  = 18   (35/2 = 17.5 -> 18)
    //   raw 30 -> med [20,900,30] = 30 -> ma [20,30]  = 25
    // The 900 spike never reaches the mean, which is the point of the prefilter.
    let mut d = harness(FilterCfg {
        ma_window: 2,
        median_window: 3,
        sample_rate_hz: 50,
        ema_alpha: 0.0,
    });
    assert_eq!(feed_all(&mut d, &[10, 20, 900, 30]), vec![10, 13, 18, 25]);
}

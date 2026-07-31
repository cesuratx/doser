//! Tests for `doser_core::runner`, the orchestration layer.
//!
//! Nothing in the workspace exercised `runner::run` before, even though it owns
//! the safety behaviour the CLI depends on: the cooperative shutdown abort, the
//! sampler stall watchdog, the Timeout-vs-MaxRuntime precedence, and the
//! out-of-band E-stop poll. Since the CLI's dose loop was folded into this
//! module, a regression here is a regression in the shipped binary.
//!
//! `runner::run` builds its `Doser` internally and does not accept a clock, so
//! these tests cannot use a virtual clock. They are kept deterministic instead
//! by making every outcome depend on *causality* rather than on how fast the
//! machine is:
//!
//! - shutdown/E-stop aborts are triggered by the scale or by a pre-set flag, and
//!   the fixtures can never complete or abort any other way;
//! - the stall watchdog fires against a scale that never succeeds, so the only
//!   competing deadline (`max_run_ms`) is 25x further away;
//! - the precedence tests pick thresholds where the Timeout and MaxRuntime
//!   conditions become true on the *same* loop iteration by construction (see
//!   `precedence_params`), so which one wins is decided purely by the flag.

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use doser_core::error::{AbortReason, DoserError};
use doser_core::runner::{RunParams, SamplingMode, ShutdownFlag, run, run_observed};
use doser_core::{Calibration, ControlCfg, FilterCfg, PredictorCfg, SafetyCfg, Timeouts};

// ── Fixtures ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Start,
    SetSpeed(u32),
    Stop,
}

/// Motor whose command sequence stays observable after `run` consumes it.
#[derive(Clone, Default)]
struct SpyMotor {
    ops: Arc<Mutex<Vec<Op>>>,
}
impl SpyMotor {
    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }
    fn count(&self, want: fn(&Op) -> bool) -> usize {
        self.ops().iter().filter(|o| want(o)).count()
    }
    fn stops(&self) -> usize {
        self.count(|o| matches!(o, Op::Stop))
    }
    fn starts(&self) -> usize {
        self.count(|o| matches!(o, Op::Start))
    }
}
impl doser_traits::Motor for SpyMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.ops.lock().unwrap().push(Op::Start);
        Ok(())
    }
    fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.ops.lock().unwrap().push(Op::SetSpeed(sps));
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.ops.lock().unwrap().push(Op::Stop);
        Ok(())
    }
}

/// Scale that always reads the same value, counts its reads, and optionally
/// raises a shared flag once it has been read `trip_after` times. Raising the
/// flag from inside `read` is what makes the mid-run shutdown tests causal
/// rather than timed.
struct CountingScale {
    value: i32,
    reads: Arc<AtomicUsize>,
    trip: Option<(Arc<AtomicBool>, usize)>,
}
impl CountingScale {
    fn new(value: i32) -> (Self, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        (
            Self {
                value,
                reads: reads.clone(),
                trip: None,
            },
            reads,
        )
    }
    fn tripping(value: i32, flag: Arc<AtomicBool>, after: usize) -> (Self, Arc<AtomicUsize>) {
        let (mut s, reads) = Self::new(value);
        s.trip = Some((flag, after));
        (s, reads)
    }
}
impl doser_traits::Scale for CountingScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        let n = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((flag, after)) = &self.trip
            && n >= *after
        {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(self.value)
    }
}

/// Scale that never produces a reading, so the sampler's `last_ok` never moves
/// and the stall watchdog is the only thing that can end the run.
struct NeverReadyScale;
impl doser_traits::Scale for NeverReadyScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        Err(Box::new(std::io::Error::other("no data ready")))
    }
}

/// Scale returning a fixed sequence of raw counts, then repeating the last one.
struct SeqScale {
    seq: Vec<i32>,
    idx: usize,
    reads: Arc<AtomicUsize>,
}
impl SeqScale {
    fn new(seq: Vec<i32>) -> (Self, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        (
            Self {
                seq,
                idx: 0,
                reads: reads.clone(),
            },
            reads,
        )
    }
}
impl doser_traits::Scale for SeqScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let v = self
            .seq
            .get(self.idx)
            .copied()
            .unwrap_or_else(|| self.seq.last().copied().unwrap_or(0));
        self.idx += 1;
        Ok(v)
    }
}

/// Calibration gain 0.01 g/count => one raw count is exactly one centigram.
fn cg_calibration() -> Calibration {
    Calibration {
        gain_g_per_count: 0.01,
        zero_counts: 0,
        offset_g: 0.0,
    }
}

/// Base parameters: a 10 g target that the fixtures below never reach, with
/// every watchdog except the one under test pushed far out of the way.
fn base_params(mode: SamplingMode, sample_rate_hz: u32) -> RunParams {
    RunParams {
        filter: FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz,
            ema_alpha: 0.0,
        },
        control: ControlCfg {
            stable_ms: 0,
            ..ControlCfg::default()
        },
        safety: SafetyCfg {
            max_run_ms: 60_000,
            max_overshoot_g: 100.0,
            no_progress_epsilon_g: 0.0,
            no_progress_ms: 0,
        },
        timeouts: Timeouts { sensor_ms: 10 },
        calibration: Some(cg_calibration()),
        target_g: 10.0,
        estop_debounce_n: 1,
        prefer_timeout_first: false,
        mode,
        predictor: None,
        shutdown: None,
    }
}

fn abort_reason(err: &eyre::Report) -> Option<AbortReason> {
    match err.downcast_ref::<DoserError>() {
        Some(DoserError::Abort(r)) => Some(r.clone()),
        _ => None,
    }
}

fn is_timeout(err: &eyre::Report) -> bool {
    matches!(err.downcast_ref::<DoserError>(), Some(DoserError::Timeout))
}

// ── Cooperative shutdown ─────────────────────────────────────────────────────

#[test]
fn shutdown_flag_set_before_the_run_aborts_direct_mode_without_reading() {
    let flag: ShutdownFlag = Arc::new(AtomicBool::new(true));
    let motor = SpyMotor::default();
    let (scale, reads) = CountingScale::new(0);

    let mut params = base_params(SamplingMode::Direct, 1_000);
    params.shutdown = Some(flag);

    let err = run(scale, motor.clone(), None, params).expect_err("shutdown must abort the run");
    assert_eq!(
        abort_reason(&err),
        Some(AbortReason::Estop),
        "shutdown must surface as an E-stop abort, got: {err:#}"
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "the flag is checked before the sample, so no read should happen"
    );
    assert!(
        motor.stops() >= 1,
        "the motor must be stopped on the shutdown path: {:?}",
        motor.ops()
    );
    assert_eq!(motor.starts(), 0, "the motor was never supposed to run");
}

#[test]
fn shutdown_flag_raised_mid_run_aborts_direct_mode_and_stops_the_motor() {
    let flag: ShutdownFlag = Arc::new(AtomicBool::new(false));
    let motor = SpyMotor::default();
    // Reads 0 g forever: the run can only end via the shutdown flag, which the
    // scale raises on its 3rd read.
    let (scale, reads) = CountingScale::tripping(0, flag.clone(), 3);

    let mut params = base_params(SamplingMode::Direct, 1_000);
    params.shutdown = Some(flag);

    let err = run(scale, motor.clone(), None, params).expect_err("shutdown must abort the run");
    assert_eq!(abort_reason(&err), Some(AbortReason::Estop), "{err:#}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        3,
        "the abort must land on the iteration after the flag was raised"
    );
    let ops = motor.ops();
    assert!(
        motor.starts() >= 1,
        "the motor should have been running before the shutdown: {ops:?}"
    );
    assert_eq!(
        ops.last(),
        Some(&Op::Stop),
        "the last command on the shutdown path must be a stop: {ops:?}"
    );
}

#[test]
fn shutdown_flag_raised_mid_run_aborts_sampler_mode_and_stops_the_motor() {
    let flag: ShutdownFlag = Arc::new(AtomicBool::new(false));
    let motor = SpyMotor::default();
    // The sampler thread owns the scale; raising the flag from inside `read`
    // means the runner loop observes it on its next pass regardless of timing.
    let (scale, _reads) = CountingScale::tripping(0, flag.clone(), 2);

    let mut params = base_params(SamplingMode::Paced(200), 200);
    params.shutdown = Some(flag);

    let err = run(scale, motor.clone(), None, params).expect_err("shutdown must abort the run");
    assert_eq!(abort_reason(&err), Some(AbortReason::Estop), "{err:#}");
    let ops = motor.ops();
    assert_eq!(
        ops.last(),
        Some(&Op::Stop),
        "sampler mode must also stop the motor on shutdown: {ops:?}"
    );
}

// ── Stall watchdog and out-of-band E-stop ────────────────────────────────────

#[test]
fn sampler_stall_watchdog_returns_timeout_and_stops_the_motor() {
    let motor = SpyMotor::default();

    // sensor_ms=10, period=50 ms (20 Hz), max_run=5 s
    //   -> stall threshold = max(4*10, 2*50) = 100 ms, capped below max_run.
    // The scale never succeeds, so the watchdog fires ~100 ms in, 25x before the
    // 5 s hard cap; nothing else can end this run.
    let mut params = base_params(SamplingMode::Paced(20), 20);
    params.safety.max_run_ms = 5_000;

    let err = run(NeverReadyScale, motor.clone(), None, params)
        .expect_err("a scale that never reads must trip the stall watchdog");
    assert!(
        is_timeout(&err),
        "expected DoserError::Timeout, got: {err:#}"
    );
    assert!(
        motor.stops() >= 1,
        "the timeout path must stop the motor: {:?}",
        motor.ops()
    );
}

#[test]
fn out_of_band_estop_poll_aborts_before_any_sample_arrives() {
    let motor = SpyMotor::default();
    // Same fixture as the stall test above, which ends in Timeout: no sample
    // ever reaches `step_from_raw`, so an E-stop seen only on the sample path
    // could never fire. Getting Estop instead of Timeout is therefore proof the
    // out-of-band poll ran.
    let mut params = base_params(SamplingMode::Paced(20), 20);
    params.safety.max_run_ms = 5_000;
    params.estop_debounce_n = 1;

    let err = run(
        NeverReadyScale,
        motor.clone(),
        Some(Box::new(|| true)),
        params,
    )
    .expect_err("a latched E-stop must abort the run");
    assert_eq!(
        abort_reason(&err),
        Some(AbortReason::Estop),
        "expected the out-of-band poll to latch E-stop, got: {err:#}"
    );
    assert!(
        motor.stops() >= 1,
        "the E-stop path must stop the motor: {:?}",
        motor.ops()
    );
}

// ── Timeout vs MaxRuntime precedence ─────────────────────────────────────────

/// Thresholds chosen so both deadlines become true on the same loop iteration.
///
/// `compute_stall_threshold_ms(sensor_ms=100, period_ms=50, max_run_ms=200)`
/// yields `min(max(4*100, 2*50), 200 - 1) = 199`, so stall fires once
/// `elapsed >= 199 && stalled > 199`, and max-run fires once `elapsed >= 200`.
///
/// With no sample ever arriving, `stalled == elapsed`, and the loop advances in
/// 50 ms sleeps from `elapsed == 0` — and `thread::sleep` never returns early —
/// so the first iteration that satisfies either condition is the one at
/// `elapsed >= 200`, where both hold. Whichever error comes back is decided
/// solely by `prefer_timeout_first`.
fn precedence_params(prefer_timeout_first: bool) -> RunParams {
    let mut params = base_params(SamplingMode::Paced(20), 20);
    params.timeouts = Timeouts { sensor_ms: 100 };
    params.safety.max_run_ms = 200;
    params.prefer_timeout_first = prefer_timeout_first;
    params
}

#[test]
fn prefer_timeout_first_reports_timeout_over_max_runtime() {
    let motor = SpyMotor::default();
    let err = run(
        NeverReadyScale,
        motor.clone(),
        None,
        precedence_params(true),
    )
    .expect_err("run must abort");
    assert!(
        is_timeout(&err),
        "prefer_timeout_first=true must report Timeout, got: {err:#}"
    );
    assert!(motor.stops() >= 1, "{:?}", motor.ops());
}

#[test]
fn without_prefer_timeout_first_max_runtime_wins() {
    let motor = SpyMotor::default();
    let err = run(
        NeverReadyScale,
        motor.clone(),
        None,
        precedence_params(false),
    )
    .expect_err("run must abort");
    assert_eq!(
        abort_reason(&err),
        Some(AbortReason::MaxRuntime),
        "prefer_timeout_first=false must report MaxRuntime, got: {err:#}"
    );
    assert!(motor.stops() >= 1, "{:?}", motor.ops());
}

// ── RunOutcome telemetry ─────────────────────────────────────────────────────

/// Raw counts (== centigrams) for the telemetry runs: a ramp, one predictor
/// firing at 9.00 g, then a small settle back to 8.95 g which takes the slope
/// non-positive so the predictor releases and the completion zone is entered.
fn telemetry_sequence() -> Vec<i32> {
    vec![100, 200, 300, 400, 500, 600, 700, 800, 880, 900, 895]
}

fn telemetry_params(predictor: Option<PredictorCfg>) -> RunParams {
    let mut params = base_params(SamplingMode::Direct, 100);
    params.target_g = 9.0;
    params.control = ControlCfg {
        stable_ms: 0,
        epsilon_g: 0.08,
        ..ControlCfg::default()
    };
    params.predictor = predictor;
    params
}

#[test]
fn run_outcome_reports_predictor_telemetry_and_the_observer_sees_every_sample() {
    let motor = SpyMotor::default();
    let (scale, reads) = SeqScale::new(telemetry_sequence());
    // A latency margin far larger than the loop period makes the in-flight
    // estimate dominate, so the predictor fires on every rising sample
    // regardless of scheduling jitter — the *sample index* at which it last
    // fires is what this test pins, not a wall-clock-dependent slope.
    let params = telemetry_params(Some(PredictorCfg {
        enabled: true,
        window: 2,
        extra_latency_ms: 100_000,
        min_progress_ratio: 0.5,
    }));

    let mut observed = 0usize;
    let mut on_step = |_d: Duration| observed += 1;
    let outcome = run_observed(scale, motor.clone(), None, params, Some(&mut on_step))
        .unwrap_or_else(|e| panic!("run: {e:#}"));

    // The 895 sample releases the predictor (dw <= 0) and enters the completion
    // zone, since 895 + 8 >= 900.
    assert!(
        (outcome.final_g - 8.95).abs() < 1e-6,
        "final_g={}, expected 8.95",
        outcome.final_g
    );
    // The last predictive stop was issued at the 900 sample, one sample earlier.
    let early = outcome
        .early_stop_at_g
        .expect("early_stop_at_g must be populated when the predictor fires");
    assert!(
        (early - 9.00).abs() < 1e-6,
        "early_stop_at_g={early}, expected 9.00"
    );
    assert!(
        early > outcome.final_g,
        "the predictive stop should precede the settle: early={early} final={}",
        outcome.final_g
    );
    let slope = outcome
        .slope_ema_gps
        .expect("slope_ema_gps must be populated when the predictor runs");
    assert!(slope > 0.0, "slope_ema_gps={slope} on a rising ramp");
    let inflight = outcome
        .inflight_g
        .expect("inflight_g must be populated when the predictor runs");
    assert!(inflight > 0.0, "inflight_g={inflight} on a rising ramp");

    assert_eq!(
        observed,
        reads.load(Ordering::SeqCst),
        "the observer must fire once per consumed sample"
    );
    assert_eq!(observed, 11, "expected the run to consume 11 samples");
    assert!(motor.stops() >= 1, "{:?}", motor.ops());
}

#[test]
fn run_outcome_omits_predictor_telemetry_when_the_predictor_is_off() {
    let motor = SpyMotor::default();
    let (scale, reads) = SeqScale::new(telemetry_sequence());
    let params = telemetry_params(None);

    let mut observed = 0usize;
    let mut on_step = |_d: Duration| observed += 1;
    let outcome = run_observed(scale, motor.clone(), None, params, Some(&mut on_step))
        .unwrap_or_else(|e| panic!("run: {e:#}"));

    // Without the predictor holding the motor off, the 900 sample enters the
    // completion zone directly, one sample earlier than the predictive run.
    assert!(
        (outcome.final_g - 9.00).abs() < 1e-6,
        "final_g={}, expected 9.00",
        outcome.final_g
    );
    assert_eq!(outcome.early_stop_at_g, None);
    assert_eq!(outcome.slope_ema_gps, None);
    assert_eq!(outcome.inflight_g, None);
    assert_eq!(observed, reads.load(Ordering::SeqCst));
    assert_eq!(observed, 10, "expected the run to consume 10 samples");
}

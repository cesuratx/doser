//! Predictive early-stop tests.
//!
//! The predictor's whole job is to stop the motor *before* the weight reaches
//! the target, so the in-flight mass lands inside the acceptance band instead of
//! overshooting. A test that only observes "the loop is still Running and the
//! weight is above X" cannot tell that apart from the predictor being disabled,
//! so every assertion here is anchored on evidence only the predictor produces:
//! an actual `Motor::stop()` call, and the `early_stop_at_g` telemetry.
//!
//! `predictor_disabled_never_stops_early` is the power check: it runs the exact
//! same ramp with `enabled: false` and asserts each of those signals is absent.

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doser_core::{Calibration, ControlCfg, Doser, DosingStatus, FilterCfg, PredictorCfg, Timeouts};
use doser_traits::clock::Clock;
use rstest::rstest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Start,
    SetSpeed(u32),
    Stop,
}

/// Motor that records the exact command sequence it received. The builder takes
/// ownership, so the recording lives behind a cloned `Arc`.
#[derive(Clone, Default)]
struct SpyMotor {
    ops: Arc<Mutex<Vec<Op>>>,
}
impl SpyMotor {
    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }
    fn stop_count(&self) -> usize {
        self.ops().iter().filter(|o| matches!(o, Op::Stop)).count()
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

/// Deterministic clock: `sleep` advances a virtual offset instead of blocking,
/// so the predictor's slope estimate depends only on the sample sequence.
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

// ── Fixture ──────────────────────────────────────────────────────────────────
//
// Everything below is chosen so the early-stop point is a closed-form integer,
// not an approximation:
//
//   gain 0.01 g/count  -> one raw count == one centigram (see fixed_point tests)
//   target 10.00 g     -> target_cg = 1000
//   epsilon 0.05 g     -> epsilon_cg = 5
//   50 Hz              -> period 20 ms, so pred_latency = 20 + 40 = 60 ms
//   ramp of 50 cg/step -> slope 150 cg over the 60 ms window = 2.5 cg/ms
//
// inflight = round(dw * latency / dt) = round(150 * 60 / 60) = 150 cg, constant.
// The predictor fires on the first sample with `w + 150 + 5 >= 1000`, i.e.
// w >= 845 cg, i.e. the k = 17 sample at 850 cg.

const TARGET_G: f32 = 10.0;
const EPSILON_G: f32 = 0.05;
const RAMP_CG_PER_STEP: i32 = 50;
/// First sample index (1-based) at which the predictor must stop the motor.
const EXPECTED_STOP_STEP: i32 = 17;
/// Weight at that sample, in grams.
const EXPECTED_STOP_G: f32 = 8.50;
/// Steady-state in-flight estimate at that point, in grams.
const EXPECTED_INFLIGHT_G: f32 = 1.50;
/// Steady-state slope: 2.5 cg/ms == 25 g/s.
const EXPECTED_SLOPE_GPS: f32 = 25.0;

fn build(predictor: PredictorCfg, motor: SpyMotor) -> Doser {
    Doser::builder()
        .with_scale(doser_core::mocks::NoopScale)
        .with_motor(motor)
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            stable_ms: 0,
            epsilon_g: EPSILON_G,
            ..ControlCfg::default()
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_calibration(Calibration {
            gain_g_per_count: 0.01,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(TARGET_G)
        .with_clock(Box::new(ManualClock::new()))
        .with_predictor(predictor)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"))
}

fn enabled_predictor() -> PredictorCfg {
    PredictorCfg {
        enabled: true,
        window: 4,
        extra_latency_ms: 40,
        min_progress_ratio: 0.02,
    }
}

#[rstest]
fn early_stop_triggers_before_target_cross() {
    let motor = SpyMotor::default();
    let mut doser = build(enabled_predictor(), motor.clone());
    doser.begin();

    // Drive the ramp until the motor is stopped, recording the weight that was
    // in effect at that sample.
    let mut stopped_at_step: Option<i32> = None;
    let mut weight_at_stop = f32::NAN;
    for k in 1..=24 {
        let status = doser
            .step_from_raw(RAMP_CG_PER_STEP * k)
            .unwrap_or_else(|e| panic!("step {k}: {e}"));
        assert!(
            matches!(status, DosingStatus::Running),
            "step {k}: expected Running, got {status:?}"
        );
        if stopped_at_step.is_none() && motor.stop_count() > 0 {
            stopped_at_step = Some(k);
            weight_at_stop = doser.last_weight();
            break;
        }
    }

    let step = stopped_at_step.expect("predictor never stopped the motor");
    assert_eq!(
        step,
        EXPECTED_STOP_STEP,
        "early stop landed on the wrong sample; ops={:?}",
        motor.ops()
    );

    // The point of the predictor: the stop is issued while the weight is still
    // meaningfully *below* the completion zone (target - epsilon), so the
    // in-flight mass has somewhere to land.
    assert!(
        weight_at_stop < TARGET_G - EPSILON_G,
        "stopped at {weight_at_stop} g, which is not below target-epsilon ({})",
        TARGET_G - EPSILON_G
    );
    assert!(
        (weight_at_stop - EXPECTED_STOP_G).abs() < 0.005,
        "expected stop at {EXPECTED_STOP_G} g, got {weight_at_stop} g"
    );

    // Telemetry must agree with the observed hardware command, and be sane.
    let reported = doser
        .early_stop_at_g()
        .expect("early_stop_at_g must be Some after a predictive stop");
    assert!(
        (reported - EXPECTED_STOP_G).abs() < 0.005,
        "early_stop_at_g={reported} g, expected {EXPECTED_STOP_G} g"
    );
    assert!(
        reported > 0.5 * TARGET_G && reported < TARGET_G,
        "early_stop_at_g={reported} g is not a sane stop point for a {TARGET_G} g target"
    );

    let inflight = doser
        .last_inflight_g()
        .expect("in-flight estimate must be populated once the predictor runs");
    assert!(
        (inflight - EXPECTED_INFLIGHT_G).abs() < 0.005,
        "inflight={inflight} g, expected {EXPECTED_INFLIGHT_G} g"
    );
    let slope = doser
        .last_slope_ema_gps()
        .expect("slope EMA must be populated once the predictor runs");
    assert!(
        (slope - EXPECTED_SLOPE_GPS).abs() < 0.05,
        "slope={slope} g/s, expected {EXPECTED_SLOPE_GPS} g/s"
    );

    // A predictive stop must not be followed by speed commands to a motor that
    // is no longer stepping.
    assert_no_speed_while_stopped(&motor.ops());
}

/// Power check for `early_stop_triggers_before_target_cross`: with the predictor
/// disabled, the identical ramp produces none of the signals that test asserts.
/// If those assertions could pass without the predictor, this test would fail.
#[rstest]
fn predictor_disabled_never_stops_early() {
    let motor = SpyMotor::default();
    let mut doser = build(
        PredictorCfg {
            enabled: false,
            ..enabled_predictor()
        },
        motor.clone(),
    );
    doser.begin();

    for k in 1..=EXPECTED_STOP_STEP {
        let status = doser
            .step_from_raw(RAMP_CG_PER_STEP * k)
            .unwrap_or_else(|e| panic!("step {k}: {e}"));
        assert!(
            matches!(status, DosingStatus::Running),
            "step {k}: expected Running, got {status:?}"
        );
    }

    // Same sample index at which the enabled run had already stopped.
    assert_eq!(
        motor.stop_count(),
        0,
        "motor was stopped without a predictor: {:?}",
        motor.ops()
    );
    assert!(
        doser.early_stop_at_g().is_none(),
        "early_stop_at_g reported {:?} with the predictor disabled",
        doser.early_stop_at_g()
    );
    assert!(doser.last_inflight_g().is_none());
    assert!(doser.last_slope_ema_gps().is_none());

    // Without the predictor the run only stops on completion-zone entry, which
    // needs w + epsilon >= target, i.e. w >= 995 cg (the k = 20 sample).
    for k in EXPECTED_STOP_STEP + 1..=19 {
        let status = doser
            .step_from_raw(RAMP_CG_PER_STEP * k)
            .unwrap_or_else(|e| panic!("step {k}: {e}"));
        assert!(matches!(status, DosingStatus::Running), "step {k}");
    }
    assert_eq!(motor.stop_count(), 0, "stopped before the completion zone");
    let status = doser
        .step_from_raw(RAMP_CG_PER_STEP * 20)
        .unwrap_or_else(|e| panic!("step 20: {e}"));
    assert!(
        matches!(status, DosingStatus::Complete),
        "expected completion at 10.00 g, got {status:?}"
    );
    assert!(motor.stop_count() >= 1, "completion must stop the motor");
    assert!(
        doser.early_stop_at_g().is_none(),
        "a completion-zone stop is not a predictive early stop"
    );
}

/// Every `set_speed` must reach a *running* motor: backends only resume stepping
/// on `start()`, so a speed command issued after a stop is lost.
fn assert_no_speed_while_stopped(ops: &[Op]) {
    let mut running = false;
    for op in ops {
        match op {
            Op::Start => running = true,
            Op::Stop => running = false,
            Op::SetSpeed(sps) => assert!(
                running || *sps == 0,
                "set_speed({sps}) issued to a stopped motor: {ops:?}"
            ),
        }
    }
}

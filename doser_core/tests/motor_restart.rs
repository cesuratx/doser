//! Regression tests for the motor-restart fix.
//!
//! `motor_stop`/`motor_stop_best_effort` clear `motor_started`, which is what
//! makes the motor restartable *within* a run. Backends only resume stepping on
//! `start()`, so with the flag left set a reading that falls back below
//! `target - epsilon` would fall through to speed selection and command speeds
//! at a motor that is not stepping — silently, until the no-progress watchdog
//! aborted a nearly-complete dose.
//!
//! Two shapes are covered, because they fail differently:
//!
//! * A settle-band dip is sensor noise, so the mass is already in the cup and
//!   the run completes either way. What regresses is the *command sequence*:
//!   `start()` is never re-issued and `set_speed()` goes to a stopped motor.
//!   `assert_no_speed_while_stopped` and the start count are the detectors.
//! * A predictor early stop that over-estimated the in-flight mass leaves the
//!   dose genuinely short. There the *outcome* regresses: the auger never turns
//!   again, so the run aborts NoProgress instead of finishing. That test drives
//!   a motor-coupled scale so the difference is real and not scripted.

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doser_core::error::{AbortReason, DoserError};
use doser_core::{
    Calibration, ControlCfg, Doser, DosingStatus, FilterCfg, PredictorCfg, SafetyCfg, Timeouts,
};
use doser_traits::clock::Clock;
use doser_traits::{Motor, Scale};
use rstest::rstest;

// ── Fixtures ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Start,
    SetSpeed(u32),
    Stop,
}

/// Shared auger state: mass only accumulates while the motor is running.
#[derive(Default)]
struct SimState {
    cg: i32,
    running: bool,
}

/// Motor that both drives `SimState` and records its command sequence.
#[derive(Clone)]
struct SimMotor {
    st: Arc<Mutex<SimState>>,
    ops: Arc<Mutex<Vec<Op>>>,
}
impl SimMotor {
    fn new(st: Arc<Mutex<SimState>>) -> Self {
        Self {
            st,
            ops: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }
    fn starts(&self) -> usize {
        self.ops().iter().filter(|o| matches!(o, Op::Start)).count()
    }
    fn stops(&self) -> usize {
        self.ops().iter().filter(|o| matches!(o, Op::Stop)).count()
    }
}
impl Motor for SimMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.st.lock().unwrap().running = true;
        self.ops.lock().unwrap().push(Op::Start);
        Ok(())
    }
    fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.ops.lock().unwrap().push(Op::SetSpeed(sps));
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.st.lock().unwrap().running = false;
        self.ops.lock().unwrap().push(Op::Stop);
        Ok(())
    }
}

/// Scale coupled to the motor: raw counts rise by `inc_cg` per read while the
/// motor runs and hold otherwise. Calibration gain 0.01 g/count makes one raw
/// count exactly one centigram.
struct SimScale {
    st: Arc<Mutex<SimState>>,
    inc_cg: i32,
}
impl Scale for SimScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        let mut st = self.st.lock().unwrap();
        if st.running {
            st.cg += self.inc_cg;
        }
        Ok(st.cg)
    }
}

/// Scale returning a fixed sequence of raw counts, then repeating the last one.
struct SeqScale {
    seq: Vec<i32>,
    idx: usize,
}
impl Scale for SeqScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        let v = self
            .seq
            .get(self.idx)
            .copied()
            .unwrap_or_else(|| self.seq.last().copied().unwrap_or(0));
        self.idx += 1;
        Ok(v)
    }
}

/// Motor spy without simulation coupling, for the scripted dip run.
#[derive(Clone, Default)]
struct SpyMotor {
    ops: Arc<Mutex<Vec<Op>>>,
}
impl SpyMotor {
    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }
    fn starts(&self) -> usize {
        self.ops().iter().filter(|o| matches!(o, Op::Start)).count()
    }
}
impl Motor for SpyMotor {
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

/// Every `set_speed` must reach a *running* motor: backends only resume stepping
/// on `start()`, so a speed command issued after a stop is silently lost.
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

fn cg_calibration() -> Calibration {
    Calibration {
        gain_g_per_count: 0.01,
        zero_counts: 0,
        offset_g: 0.0,
    }
}

// ── Settle-band dip ──────────────────────────────────────────────────────────

#[rstest]
fn a_dip_out_of_the_settle_band_restarts_the_motor_and_the_dose_completes() {
    // Raw counts == centigrams; target 10.00 g, epsilon 0.08 g so the completion
    // zone starts at 992 cg, and the acceptance half-band is max(7, 8) = 8 cg.
    //
    //   500, 800 -> below the zone, motor runs
    //   995      -> zone entry: motor stops, settle timer starts
    //   970      -> a noisy dip 3 cg *below* the zone: the loop must restart
    //   995...   -> back in band; completes once stable_ms has elapsed
    let motor = SpyMotor::default();
    let mut doser = Doser::builder()
        .with_scale(SeqScale {
            seq: vec![500, 800, 995, 970, 995],
            idx: 0,
        })
        .with_motor(motor.clone())
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50, // 20 ms period on the manual clock
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            stable_ms: 100,
            epsilon_g: 0.08,
            hysteresis_g: 0.07,
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 60_000,
            max_overshoot_g: 2.0,
            no_progress_epsilon_g: 0.01,
            no_progress_ms: 200,
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_calibration(cg_calibration())
        .with_target_grams(10.0)
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));
    doser.begin();

    let mut status = DosingStatus::Running;
    for k in 1..=64 {
        status = doser.step().unwrap_or_else(|e| panic!("step {k}: {e}"));
        if !matches!(status, DosingStatus::Running) {
            break;
        }
    }
    assert!(
        matches!(status, DosingStatus::Complete),
        "expected the dose to complete after the dip, got {status:?}"
    );
    assert!(
        (doser.last_weight() - 9.95).abs() < 1e-6,
        "final weight {}",
        doser.last_weight()
    );

    let ops = motor.ops();
    assert_eq!(
        motor.starts(),
        2,
        "the motor must be restarted after leaving the settle band: {ops:?}"
    );
    assert_no_speed_while_stopped(&ops);
}

// ── Predictor early stop that plateaus short ─────────────────────────────────

#[rstest]
fn a_predictor_stop_that_plateaus_short_resumes_and_finishes() {
    // A motor-coupled auger delivering 50 cg per sample. With a 200 ms in-flight
    // latency the predictor's estimate is a constant 500 cg while the ramp is
    // steady, so it stops the motor at 5.00 g — half the 10.00 g target. The
    // mass then plateaus (the motor really is off), the slope decays, the
    // predictor releases, and the loop has to restart the motor to finish.
    //
    // Without the restart the auger never turns again and the no-progress
    // watchdog configured below aborts the run instead.
    let st = Arc::new(Mutex::new(SimState::default()));
    let motor = SimMotor::new(st.clone());
    let mut doser = Doser::builder()
        .with_scale(SimScale {
            st: st.clone(),
            inc_cg: 50,
        })
        .with_motor(motor.clone())
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50, // 20 ms period => predictor latency 20 + 180 ms
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            stable_ms: 0,
            epsilon_g: 0.08,
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: 60_000,
            max_overshoot_g: 2.0,
            no_progress_epsilon_g: 0.01,
            no_progress_ms: 200,
        })
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_calibration(cg_calibration())
        .with_target_grams(10.0)
        .with_predictor(PredictorCfg {
            enabled: true,
            window: 4,
            extra_latency_ms: 180,
            min_progress_ratio: 0.03,
        })
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"));
    doser.begin();

    let mut status = DosingStatus::Running;
    let mut weight_at_first_stop = f32::NAN;
    for k in 1..=200 {
        status = doser.step().unwrap_or_else(|e| panic!("step {k}: {e}"));
        if weight_at_first_stop.is_nan() && motor.stops() > 0 {
            weight_at_first_stop = doser.last_weight();
        }
        if !matches!(status, DosingStatus::Running) {
            break;
        }
    }

    match status {
        DosingStatus::Complete => {}
        DosingStatus::Aborted(DoserError::Abort(AbortReason::NoProgress)) => panic!(
            "the motor was not restarted after the predictor stop, so the dose \
             stalled short of target: {:?}",
            motor.ops()
        ),
        other => panic!("expected Complete, got {other:?}"),
    }

    // The predictor really did stop short of the completion zone, so the restart
    // was load-bearing rather than cosmetic.
    assert!(
        weight_at_first_stop < 10.0 - 0.08,
        "first predictive stop at {weight_at_first_stop} g is not short of the completion zone"
    );
    assert!(
        (weight_at_first_stop - 5.00).abs() < 1e-6,
        "expected the first predictive stop at 5.00 g, got {weight_at_first_stop}"
    );
    assert!(
        (doser.last_weight() - 10.00).abs() < 1e-6,
        "final weight {}",
        doser.last_weight()
    );

    let ops = motor.ops();
    assert!(
        motor.starts() >= 2,
        "the motor must be restarted after the predictor releases: {ops:?}"
    );
    assert_no_speed_while_stopped(&ops);
}

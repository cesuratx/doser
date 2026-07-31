//! Property tests for the safety invariants of the control loop.
//!
//! Two things were wrong here before and are worth recording, because both
//! failure modes are invisible in a green test run:
//!
//! 1. The overshoot invariant was `final_g <= target + max_overshoot_g`, but
//!    `process_weight` fires that abort exactly when `w > target + overshoot`
//!    and reports that same `w` as the final weight. The assertion was therefore
//!    unsatisfiable whenever the branch ran. What is actually true — and what is
//!    worth guaranteeing — is that the abort fires on the *first* sample past
//!    the limit, so the weight can only be carried one sample-step past the
//!    point where the completion zone would have been entered.
//! 2. That branch was also effectively unreachable under the generated inputs,
//!    and the run-terminating `prop_assert!(done || aborted_no_progress)` would
//!    have failed if it ever had been. `overshoot_abort_is_bounded_to_one_sample`
//!    below constructs inputs where the branch is guaranteed to run.
//!
//! Both properties drive a deterministic clock, so a case that fails fails for
//! the generated input rather than for the CI runner's scheduling.

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doser_core::error::{AbortReason, DoserError};
use doser_core::{Calibration, ControlCfg, Doser, DosingStatus, FilterCfg, SafetyCfg, Timeouts};
use doser_traits::clock::Clock;
use proptest::prelude::*;

// ── Fixture ──────────────────────────────────────────────────────────────────

/// Calibration gain of 0.01 g/count makes one raw count exactly one centigram,
/// so every threshold below can be reasoned about in exact integers.
const EPSILON_G: f32 = 0.08;
const EPSILON_CG: i32 = 8;
const MAX_OVERSHOOT_G: f32 = 0.10;
const MAX_OVERSHOOT_CG: i32 = 10;
/// 500 Hz -> 2 ms loop period on the manual clock.
const SAMPLE_RATE_HZ: u32 = 500;
const MAX_RUN_MS: u64 = 5_000;

/// Scale that walks up by `step_cg` per read for `rising_reads` reads and then
/// holds its value, modelling an auger that empties mid-dose.
struct RampThenStallScale {
    cg: i32,
    step_cg: i32,
    rising_reads: usize,
    reads: usize,
}
impl doser_traits::Scale for RampThenStallScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        if self.reads < self.rising_reads {
            self.cg = self.cg.saturating_add(self.step_cg);
        }
        self.reads += 1;
        Ok(self.cg)
    }
}

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

/// How a run ended, with the terminal weight in centigrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Complete,
    NoProgress,
    Overshoot,
    Other,
}

fn build(target_cg: i32, step_cg: i32, rising_reads: usize) -> Doser {
    Doser::builder()
        .with_scale(RampThenStallScale {
            cg: 0,
            step_cg,
            rising_reads,
            reads: 0,
        })
        .with_motor(NoopMotor)
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: SAMPLE_RATE_HZ,
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            stable_ms: 0,
            epsilon_g: EPSILON_G,
            ..ControlCfg::default()
        })
        .with_safety(SafetyCfg {
            max_run_ms: MAX_RUN_MS,
            max_overshoot_g: MAX_OVERSHOOT_G,
            no_progress_epsilon_g: 0.005,
            no_progress_ms: 10,
        })
        .with_timeouts(Timeouts { sensor_ms: 10 })
        .with_calibration(Calibration {
            gain_g_per_count: 0.01,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams((target_cg as f32) / 100.0)
        .with_clock(Box::new(ManualClock::new()))
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build doser: {e}"))
}

/// Run to termination; returns the outcome and the final weight in centigrams.
fn drive(doser: &mut Doser) -> Option<(Outcome, i32)> {
    doser.begin();
    for _ in 0..5_000 {
        let status = match doser.step() {
            Ok(s) => s,
            Err(_) => return None,
        };
        let outcome = match status {
            DosingStatus::Running => continue,
            DosingStatus::Complete => Outcome::Complete,
            DosingStatus::Aborted(DoserError::Abort(AbortReason::NoProgress)) => {
                Outcome::NoProgress
            }
            DosingStatus::Aborted(DoserError::Abort(AbortReason::Overshoot)) => Outcome::Overshoot,
            DosingStatus::Aborted(_) => Outcome::Other,
        };
        return Some((outcome, (doser.last_weight() * 100.0).round() as i32));
    }
    None
}

// ── Properties ───────────────────────────────────────────────────────────────

proptest! {
    /// General shape: whichever way a run ends, the terminal weight is where
    /// that ending says it should be.
    ///
    /// The target is generated *relative to the ramp* (`samples_before` steps
    /// plus a small offset) rather than independently. With an independent
    /// target the ramp almost never reached the completion zone at all — about
    /// 95% of cases ended in NoProgress and only ~0.4% in Overshoot, so the
    /// overshoot arm below was decorative. This shape lands roughly 40/37/23 on
    /// Complete/NoProgress/Overshoot, so every arm is exercised each run.
    #[test]
    fn terminal_weight_matches_the_terminating_condition(
        step_cg in 1i32..61,
        samples_before in 3i32..40,
        target_offset_cg in -30i32..40,
        rising_reads in 1usize..60,
    ) {
        // Clamped to the builder's minimum sensible target (1.00 g).
        let target_cg = (samples_before * step_cg + target_offset_cg).max(100);
        let mut doser = build(target_cg, step_cg, rising_reads);
        let (outcome, final_cg) = drive(&mut doser)
            .ok_or_else(|| TestCaseError::fail("run did not terminate"))?;

        match outcome {
            // Completion means the first sample to reach the zone landed inside
            // the acceptance window rather than past the overshoot limit.
            Outcome::Complete => {
                prop_assert!(
                    final_cg >= target_cg - EPSILON_CG,
                    "completed at {final_cg} cg, below the zone entry {}",
                    target_cg - EPSILON_CG
                );
                prop_assert!(
                    final_cg <= target_cg + MAX_OVERSHOOT_CG,
                    "completed at {final_cg} cg, past the overshoot limit {}",
                    target_cg + MAX_OVERSHOOT_CG
                );
            }
            // The abort fires on the first sample past the limit, so the weight
            // is strictly past it, and by at most one sample-step beyond the
            // point where the completion zone would have been entered.
            Outcome::Overshoot => {
                prop_assert!(
                    final_cg > target_cg + MAX_OVERSHOOT_CG,
                    "overshoot abort at {final_cg} cg is not past the limit {}",
                    target_cg + MAX_OVERSHOOT_CG
                );
                prop_assert!(
                    final_cg < target_cg - EPSILON_CG + step_cg,
                    "overshoot abort at {final_cg} cg carried more than one {step_cg} cg \
                     sample past the zone entry {}",
                    target_cg - EPSILON_CG
                );
            }
            // The watchdog only runs below the completion zone; inside it the
            // loop returns before reaching the watchdog at all.
            Outcome::NoProgress => {
                prop_assert!(
                    final_cg < target_cg - EPSILON_CG,
                    "no-progress abort at {final_cg} cg, which is inside the completion zone"
                );
            }
            Outcome::Other => {
                prop_assert!(false, "unexpected abort at {final_cg} cg");
            }
        }
    }

    /// Constructed so the overshoot branch is *guaranteed* to run: the sample
    /// before the target lands 9 cg short of the completion zone's entry, and
    /// the step is large enough (>= 20 cg > epsilon + max_overshoot = 18 cg)
    /// that the next one clears the abort line in a single jump.
    #[test]
    fn overshoot_abort_is_bounded_to_one_sample(
        step_cg in 20i32..81,
        samples_before in 2i32..21,
    ) {
        // Last sample below the zone sits at target_cg - 9.
        let target_cg = samples_before * step_cg + 9;
        let mut doser = build(target_cg, step_cg, 200);
        let (outcome, final_cg) = drive(&mut doser)
            .ok_or_else(|| TestCaseError::fail("run did not terminate"))?;

        prop_assert_eq!(
            outcome,
            Outcome::Overshoot,
            "expected an overshoot abort for target={} cg step={} cg, got {:?} at {} cg",
            target_cg, step_cg, outcome, final_cg
        );
        prop_assert_eq!(
            final_cg,
            target_cg - 9 + step_cg,
            "abort did not fire on the first sample past the limit"
        );
        prop_assert!(final_cg > target_cg + MAX_OVERSHOOT_CG);
        prop_assert!(final_cg < target_cg - EPSILON_CG + step_cg);
    }
}

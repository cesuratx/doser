use crate::error::{DoserError, Result as CoreResult};
use crate::sampler::Sampler;
use crate::{Calibration, ControlCfg, DosingStatus, FilterCfg, SafetyCfg, Timeouts};
use doser_traits::clock::MonotonicClock;
use std::time::Duration;

/// How sampling should be orchestrated
#[derive(Debug, Clone, Copy)]
pub enum SamplingMode {
    /// Read inside control loop using Scale::read(timeout)
    Direct,
    /// Event-driven: block on sensor DRDY via Scale::read(timeout)
    Event,
    /// Rate-paced sampling at given Hz
    Paced(u32),
}

/// Compute the stall watchdog threshold in milliseconds.
///
/// Parameters:
/// - `sensor_timeout_ms`: the per-read sensor timeout in milliseconds. Expected ≥ 1.
///   Used to derive a conservative "fast" stall threshold (4x timeout) for quick detection.
/// - `period_ms`: the sampling period in milliseconds derived from `sample_rate_hz`.
///   Expected ≥ 1 (clamped by utility helpers); used to ensure the threshold spans at least
///   two periods so that a single missed sample doesn't immediately trip the watchdog.
/// - `max_run_ms`: configured hard cap for a dosing run. Expected ≥ 1. The stall threshold
///   is kept strictly below this cap when `max_run_ms` is smaller than two periods to avoid
///   underflow and guarantee the stall watchdog can still fire before the hard cap.
///
/// Rationale:
/// - Start from a "fast" threshold based on the sensor timeout (4x) to catch stalls promptly.
/// - Ensure the threshold is not shorter than two sampling periods to allow at least
///   one missed sample without tripping (safe threshold).
/// - If the configured `max_run_ms` is smaller than two periods, cap the threshold below
///   `max_run_ms` to avoid underflow and ensure the watchdog can still trip before the
///   hard max runtime; always return at least 1 ms.
#[inline]
fn compute_stall_threshold_ms(sensor_timeout_ms: u64, period_ms: u64, max_run_ms: u64) -> u64 {
    let fast_threshold = sensor_timeout_ms.saturating_mul(4);
    let safe_threshold = std::cmp::max(fast_threshold, period_ms.saturating_mul(2));
    if max_run_ms < period_ms.saturating_mul(2) {
        fast_threshold.min(max_run_ms.saturating_sub(1)).max(1)
    } else {
        safe_threshold
    }
}

#[inline]
fn stalled_now(elapsed_ms: u64, stalled_ms: u64, threshold_ms: u64) -> bool {
    elapsed_ms >= threshold_ms && stalled_ms > threshold_ms
}

/// Run the controller until completion or abort, returning final grams on success.
/// The caller should pre-merge any safety overrides (e.g., max_run_ms) into `safety`.
#[allow(clippy::too_many_arguments)]
pub fn run<S, M>(
    scale: S,
    motor: M,
    filter: FilterCfg,
    control: ControlCfg,
    safety: SafetyCfg,
    timeouts: Timeouts,
    calibration: Option<Calibration>,
    target_g: f32,
    estop_check: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    estop_debounce_n: u8,
    prefer_timeout_first: bool,
    mode: SamplingMode,
) -> CoreResult<f32>
where
    S: doser_traits::Scale + Send + 'static,
    M: doser_traits::Motor + 'static,
{
    match mode {
        SamplingMode::Direct => run_direct(
            scale,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            estop_check,
            estop_debounce_n,
        ),
        SamplingMode::Event | SamplingMode::Paced(_) => run_with_sampler(
            scale,
            motor,
            filter,
            control,
            safety,
            timeouts,
            calibration,
            target_g,
            estop_check,
            estop_debounce_n,
            prefer_timeout_first,
            mode,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_direct<S, M>(
    scale: S,
    motor: M,
    filter: FilterCfg,
    control: ControlCfg,
    safety: SafetyCfg,
    timeouts: Timeouts,
    calibration: Option<Calibration>,
    target_g: f32,
    estop_check: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    estop_debounce_n: u8,
) -> CoreResult<f32>
where
    S: doser_traits::Scale + 'static,
    M: doser_traits::Motor + 'static,
{
    let estop_check_core: Option<Box<dyn Fn() -> bool>> =
        estop_check.map(|f| -> Box<dyn Fn() -> bool> { Box::new(f) });
    let mut doser = crate::build_doser(
        scale,
        motor,
        filter,
        control,
        safety,
        timeouts,
        calibration,
        target_g,
        estop_check_core,
        None,
        Some(estop_debounce_n),
    )?;
    doser.begin();
    tracing::info!(target_g, mode = "direct", "dose start");

    loop {
        match doser.step()? {
            DosingStatus::Running => continue,
            DosingStatus::Complete => {
                let final_g = doser.last_weight();
                tracing::info!(final_g, "dose complete");
                return Ok(final_g);
            }
            DosingStatus::Aborted(e) => {
                let _ = doser.motor_stop();
                tracing::error!(error = %e, "dose aborted");
                return Err(crate::error::Report::new(e));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_with_sampler<S, M>(
    scale: S,
    motor: M,
    filter: FilterCfg,
    control: ControlCfg,
    safety: SafetyCfg,
    timeouts: Timeouts,
    calibration: Option<Calibration>,
    target_g: f32,
    estop_check: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    estop_debounce_n: u8,
    prefer_timeout_first: bool,
    mode: SamplingMode,
) -> CoreResult<f32>
where
    S: doser_traits::Scale + Send + 'static,
    M: doser_traits::Motor + 'static,
{
    // Use shared NoopScale since step_from_raw won't call read()
    use crate::mocks::NoopScale;

    let period_us = crate::util::period_us(filter.sample_rate_hz);
    let period_ms = crate::util::period_ms(filter.sample_rate_hz);
    // Bound stall threshold by max_run_ms to avoid underflow
    let stall_threshold_ms =
        compute_stall_threshold_ms(timeouts.sensor_ms, period_ms, safety.max_run_ms);

    let sampler_timeout = Duration::from_millis(timeouts.sensor_ms);
    let sampler = match mode {
        SamplingMode::Event => Sampler::spawn_event(scale, sampler_timeout, MonotonicClock::new()),
        SamplingMode::Paced(hz) => {
            Sampler::spawn(scale, hz, sampler_timeout, MonotonicClock::new())
        }
        SamplingMode::Direct => unreachable!(),
    };

    // Convert checker to core type
    let estop_check_core: Option<Box<dyn Fn() -> bool>> =
        estop_check.map(|f| -> Box<dyn Fn() -> bool> { Box::new(f) });

    // Build controller with NoopScale; it will only receive samples via step_from_raw
    let mut doser = crate::build_doser(
        NoopScale,
        motor,
        filter.clone(),
        control.clone(),
        safety.clone(),
        timeouts.clone(),
        calibration,
        target_g,
        estop_check_core,
        None,
        Some(estop_debounce_n),
    )?;
    doser.begin();

    tracing::info!(target_g, mode = "sampler", "dose start");

    let start = std::time::Instant::now();
    loop {
        let elapsed_ms: u64 = {
            let ms = start.elapsed().as_millis();
            (ms.min(u128::from(u64::MAX))) as u64
        };
        // Timeout vs max-run precedence
        let stalled_ms = sampler.stalled_for_now();
        if prefer_timeout_first && stalled_now(elapsed_ms, stalled_ms, stall_threshold_ms) {
            let _ = doser.motor_stop();
            return Err(crate::error::Report::new(DoserError::Timeout));
        }

        // Max run enforcement
        if elapsed_ms >= safety.max_run_ms {
            let _ = doser.motor_stop();
            return Err(crate::error::Report::new(DoserError::State(
                "max run time exceeded".into(),
            )));
        }

        if !prefer_timeout_first && stalled_now(elapsed_ms, stalled_ms, stall_threshold_ms) {
            let _ = doser.motor_stop();
            return Err(crate::error::Report::new(DoserError::Timeout));
        }

        if let Some(raw) = sampler.latest() {
            match doser.step_from_raw(raw)? {
                DosingStatus::Running => continue,
                DosingStatus::Complete => {
                    let final_g = doser.last_weight();
                    tracing::info!(final_g, "dose complete");
                    return Ok(final_g);
                }
                DosingStatus::Aborted(e) => {
                    let _ = doser.motor_stop();
                    tracing::error!(error = %e, "dose aborted");
                    return Err(crate::error::Report::new(e));
                }
            }
        } else {
            // avoid busy spin if no sample yet
            std::thread::sleep(Duration::from_micros(period_us));
        }
    }
}

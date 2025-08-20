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

    let period_us = 1_000_000u64 / u64::from(filter.sample_rate_hz);
    let _period_ms = (1000u64 / u64::from(filter.sample_rate_hz)).max(1);
    let fast_threshold = timeouts.sensor_ms.saturating_mul(4);
    let safe_threshold = std::cmp::max(fast_threshold, _period_ms.saturating_mul(2));
    // Bound stall threshold by max_run_ms to avoid underflow
    let stall_threshold_ms = if safety.max_run_ms < _period_ms.saturating_mul(2) {
        fast_threshold
            .min(safety.max_run_ms.saturating_sub(1))
            .max(1)
    } else {
        safe_threshold
    };

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
        // Timeout vs max-run precedence
        if prefer_timeout_first
            && start.elapsed().as_millis() as u64 >= stall_threshold_ms
            && sampler.stalled_for_now() > stall_threshold_ms
        {
            let _ = doser.motor_stop();
            return Err(crate::error::Report::new(DoserError::Timeout));
        }

        // Max run enforcement
        if start.elapsed().as_millis() as u64 >= safety.max_run_ms {
            let _ = doser.motor_stop();
            return Err(crate::error::Report::new(DoserError::State(
                "max run time exceeded".into(),
            )));
        }

        if !prefer_timeout_first
            && start.elapsed().as_millis() as u64 >= stall_threshold_ms
            && sampler.stalled_for_now() > stall_threshold_ms
        {
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

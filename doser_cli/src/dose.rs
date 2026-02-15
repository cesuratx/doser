//! Core dosing logic: config mapping, hardware assembly, and dose execution.

use crate::cli::{CliSafety, JsonTelemetry, LAST_SAFETY, RtLock};
use crate::rt::setup_rt_once;
use doser_config::Calibration;
use doser_core::error::Result as CoreResult;
use doser_core::runner::{RunParams, SamplingMode};

pub fn abort_reason_name(r: &doser_core::error::AbortReason) -> &'static str {
    use doser_core::error::AbortReason::*;
    match r {
        Estop => "Estop",
        NoProgress => "NoProgress",
        MaxRuntime => "MaxRuntime",
        Overshoot => "Overshoot",
        MaxAttempts => "MaxAttempts",
    }
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn run_dose(
    _cfg: &doser_config::Config,
    calib: Option<&Calibration>,
    grams: f32,
    max_run_ms_override: Option<u64>,
    max_overshoot_g_override: Option<f32>,
    direct: bool,
    hw: (
        impl doser_traits::Scale + Send + 'static,
        impl doser_traits::Motor + 'static,
    ),
    rt: bool,
    rt_prio: Option<i32>,
    rt_lock: Option<RtLock>,
    rt_cpu: Option<usize>,
    stats: bool,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> CoreResult<(f32, JsonTelemetry)> {
    // Real-time mode setup (Linux/macOS) — run once per process
    #[cfg(target_os = "linux")]
    {
        let mode = rt_lock.unwrap_or(RtLock::os_default());
        setup_rt_once(rt, rt_prio, mode, rt_cpu);
    }
    #[cfg(target_os = "macos")]
    {
        let mode = rt_lock.unwrap_or(RtLock::os_default());
        let _rt_prio = rt_prio; // silence unused on non-Linux builds
        let _rt_cpu = rt_cpu; // silence unused on non-Linux builds
        setup_rt_once(rt, mode);
    }

    // Stats: control loop latency, jitter, missed deadlines
    let mut latencies = Vec::new();
    let mut missed_deadlines = 0;
    let mut sample_count = 0;

    // Builder/config mapping — use From impls from doser_core::conversions
    let filter: doser_core::FilterCfg = (&_cfg.filter).into();
    let control: doser_core::ControlCfg = (&_cfg.control).into();
    let timeouts: doser_core::Timeouts = (&_cfg.timeouts).into();
    let defaults = doser_core::SafetyCfg::default();
    let mut safety: doser_core::SafetyCfg = (&_cfg.safety).into();
    // Apply CLI overrides
    if let Some(ms) = max_run_ms_override {
        safety.max_run_ms = ms;
    } else if safety.max_run_ms == 0 {
        safety.max_run_ms = defaults.max_run_ms;
    }
    if let Some(g) = max_overshoot_g_override {
        safety.max_overshoot_g = g;
    } else if safety.max_overshoot_g == 0.0 {
        safety.max_overshoot_g = defaults.max_overshoot_g;
    }
    let _ = LAST_SAFETY.set(CliSafety {
        max_run_ms: safety.max_run_ms,
        max_overshoot_g: safety.max_overshoot_g,
        no_progress_ms: safety.no_progress_ms,
        no_progress_epsilon_g: safety.no_progress_epsilon_g,
    });
    let calibration_core = calib.map(doser_core::Calibration::from);
    let (scale, motor) = hw;
    let estop_check: Option<Box<dyn Fn() -> bool + Send + Sync>> = {
        #[cfg(all(feature = "hardware", target_os = "linux"))]
        {
            if let Some(pin) = _cfg.pins.estop_in {
                match doser_hardware::make_estop_checker(
                    pin,
                    _cfg.estop.active_low,
                    _cfg.estop.poll_ms,
                ) {
                    Ok(c) => {
                        tracing::info!(
                            pin,
                            active_low = _cfg.estop.active_low,
                            poll_ms = _cfg.estop.poll_ms,
                            "E-stop enabled"
                        );
                        Some(c)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to init E-stop; continuing without it");
                        None
                    }
                }
            } else {
                None
            }
        }
        #[cfg(not(all(feature = "hardware", target_os = "linux")))]
        {
            let _ = &_cfg; // silence unused
            None
        }
    };
    let sampling_mode = if direct {
        SamplingMode::Direct
    } else {
        #[cfg(all(feature = "hardware", target_os = "linux"))]
        {
            SamplingMode::Event
        }
        #[cfg(not(all(feature = "hardware", target_os = "linux")))]
        {
            SamplingMode::Paced(_cfg.filter.sample_rate_hz)
        }
    };
    let prefer_timeout_first = max_run_ms_override.is_none();

    // Map predictor config
    let predictor_core: doser_core::PredictorCfg = (&_cfg.predictor).into();

    #[inline]
    fn record_sample(
        latencies: &mut Vec<u64>,
        missed_deadlines: &mut usize,
        period_us: u64,
        t_start: std::time::Instant,
    ) {
        let latency = t_start.elapsed().as_micros() as u64;
        latencies.push(latency);
        if latency > period_us {
            *missed_deadlines = missed_deadlines.saturating_add(1);
        }
    }

    // Stats collection for direct mode
    if matches!(sampling_mode, SamplingMode::Direct) && stats {
        // Direct mode: wrap control loop manually
        let estop_check_core: Option<Box<dyn Fn() -> bool>> =
            estop_check.map(|f| -> Box<dyn Fn() -> bool> { Box::new(f) });
        let mut doser = doser_core::build_doser(
            scale,
            motor,
            filter.clone(),
            control.clone(),
            safety.clone(),
            timeouts.clone(),
            calibration_core.clone(),
            grams,
            estop_check_core,
            Some(predictor_core.clone()),
            None,
            Some(_cfg.estop.debounce_n),
        )?;
        doser.begin();
        tracing::info!(target_g = grams, mode = "direct", "dose start");
        // Compute expected period only when collecting stats
        let period_us = doser_core::util::period_us(_cfg.filter.sample_rate_hz);
        loop {
            // Check for shutdown signal
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = doser.motor_stop();
                return Err(doser_core::error::DoserError::Abort(
                    doser_core::error::AbortReason::Estop,
                )
                .into());
            }

            let t_start = std::time::Instant::now();
            let status = doser.step()?;
            record_sample(&mut latencies, &mut missed_deadlines, period_us, t_start);
            sample_count += 1;
            match status {
                doser_core::DosingStatus::Running => continue,
                doser_core::DosingStatus::Complete => {
                    let final_g = doser.last_weight();
                    tracing::info!(final_g, "dose complete");
                    if stats && !latencies.is_empty() {
                        print_stats(
                            &latencies,
                            sample_count,
                            missed_deadlines,
                            _cfg.filter.sample_rate_hz,
                        );
                    }
                    let tel = JsonTelemetry {
                        slope_ema_gps: doser.last_slope_ema_gps(),
                        stop_at_g: doser.early_stop_at_g(),
                        coast_comp_g: doser.last_inflight_g(),
                    };
                    return Ok((final_g, tel));
                }
                doser_core::DosingStatus::Aborted(e) => {
                    let _ = doser.motor_stop();
                    tracing::error!(error = %e, "dose aborted");
                    return Err(e.into());
                }
            }
        }
    } else if stats {
        // Sampler mode: wrap control loop manually
        let period_us = doser_core::util::period_us(_cfg.filter.sample_rate_hz);
        let sampler_timeout = std::time::Duration::from_millis(timeouts.sensor_ms);
        let sampler = match sampling_mode {
            SamplingMode::Event => doser_core::sampler::Sampler::spawn_event(
                scale,
                sampler_timeout,
                doser_traits::clock::MonotonicClock::new(),
            ),
            SamplingMode::Paced(hz) => doser_core::sampler::Sampler::spawn(
                scale,
                hz,
                sampler_timeout,
                doser_traits::clock::MonotonicClock::new(),
            ),
            SamplingMode::Direct => unreachable!(),
        };
        let estop_check_core: Option<Box<dyn Fn() -> bool>> =
            estop_check.map(|f| -> Box<dyn Fn() -> bool> { Box::new(f) });
        // Use shared NoopScale for sampler-driven mode
        use doser_core::mocks::NoopScale;
        let mut doser = doser_core::build_doser(
            NoopScale,
            motor,
            filter.clone(),
            control.clone(),
            safety.clone(),
            timeouts.clone(),
            calibration_core.clone(),
            grams,
            estop_check_core,
            Some(predictor_core.clone()),
            None,
            Some(_cfg.estop.debounce_n),
        )?;
        doser.begin();
        tracing::info!(target_g = grams, mode = "sampler", "dose start");
        loop {
            // Check for shutdown signal
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = doser.motor_stop();
                return Err(doser_core::error::DoserError::Abort(
                    doser_core::error::AbortReason::Estop,
                )
                .into());
            }

            let t_start = std::time::Instant::now();
            let status = if let Some(raw) = sampler.latest() {
                sample_count += 1;
                doser.step_from_raw(raw)?
            } else {
                std::thread::sleep(std::time::Duration::from_micros(period_us));
                continue;
            };
            record_sample(&mut latencies, &mut missed_deadlines, period_us, t_start);
            match status {
                doser_core::DosingStatus::Running => continue,
                doser_core::DosingStatus::Complete => {
                    let final_g = doser.last_weight();
                    tracing::info!(final_g, "dose complete");
                    if stats && !latencies.is_empty() {
                        print_stats(
                            &latencies,
                            sample_count,
                            missed_deadlines,
                            _cfg.filter.sample_rate_hz,
                        );
                    }
                    let tel = JsonTelemetry {
                        slope_ema_gps: doser.last_slope_ema_gps(),
                        stop_at_g: doser.early_stop_at_g(),
                        coast_comp_g: doser.last_inflight_g(),
                    };
                    return Ok((final_g, tel));
                }
                doser_core::DosingStatus::Aborted(e) => {
                    let _ = doser.motor_stop();
                    tracing::error!(error = %e, "dose aborted");
                    return Err(e.into());
                }
            }
        }
    } else {
        // No stats: use core runner
        let final_g = doser_core::runner::run(
            scale,
            motor,
            estop_check,
            RunParams {
                filter,
                control,
                safety,
                timeouts,
                calibration: calibration_core,
                target_g: grams,
                estop_debounce_n: _cfg.estop.debounce_n,
                prefer_timeout_first,
                mode: sampling_mode,
                predictor: Some(predictor_core),
            },
        )?;
        // Telemetry not available through runner; return nulls
        let tel = JsonTelemetry::default();
        return Ok((final_g, tel));
    }
    // Unreachable
    #[allow(unreachable_code)]
    Ok((0.0, JsonTelemetry::default()))
}

/// Print latency/jitter stats to stderr.
fn print_stats(
    latencies: &[u64],
    sample_count: usize,
    missed_deadlines: usize,
    sample_rate_hz: u32,
) {
    let expected_period_us = doser_core::util::period_us(sample_rate_hz);
    let min = *latencies.iter().min().unwrap_or(&0);
    let max = *latencies.iter().max().unwrap_or(&0);
    let avg = latencies.iter().sum::<u64>() as f64 / latencies.len() as f64;
    let stdev = if latencies.len() > 1 {
        let mean = avg;
        let var = latencies
            .iter()
            .map(|&x| (x as f64 - mean).powi(2))
            .sum::<f64>()
            / (latencies.len() as f64 - 1.0);
        var.sqrt()
    } else {
        0.0
    };
    eprintln!("\n--- Doser Stats ---");
    eprintln!("Samples: {sample_count}");
    eprintln!("Period (us): {expected_period_us}");
    eprintln!("Latency min/avg/max/stdev (us): {min:.0} / {avg:.1} / {max:.0} / {stdev:.1}");
    eprintln!("Missed deadlines (> period): {missed_deadlines}");
    eprintln!("-------------------\n");
}

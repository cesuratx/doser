//! Core dosing logic: config mapping, hardware assembly, and dose execution.

use crate::cli::{CliSafety, JsonTelemetry, LAST_SAFETY, RtLock};
use crate::rt::setup_rt_once;
use doser_config::Calibration;
use doser_core::error::Result as CoreResult;
use doser_core::runner::{RunParams, SamplingMode};

#[must_use]
pub const fn abort_reason_name(r: &doser_core::error::AbortReason) -> &'static str {
    use doser_core::error::AbortReason::{Estop, MaxAttempts, MaxRuntime, NoProgress, Overshoot};
    match r {
        Estop => "Estop",
        NoProgress => "NoProgress",
        MaxRuntime => "MaxRuntime",
        Overshoot => "Overshoot",
        MaxAttempts => "MaxAttempts",
    }
}

/// Build the E-stop checker from config, when the platform and config provide one.
fn make_estop_check(cfg: &doser_config::Config) -> Option<Box<dyn Fn() -> bool + Send + Sync>> {
    #[cfg(all(feature = "hardware", target_os = "linux"))]
    {
        if let Some(pin) = cfg.pins.estop_in {
            match doser_hardware::make_estop_checker(pin, cfg.estop.active_low, cfg.estop.poll_ms) {
                Ok(c) => {
                    tracing::info!(
                        pin,
                        active_low = cfg.estop.active_low,
                        poll_ms = cfg.estop.poll_ms,
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
        let _ = cfg; // no GPIO backend on this target
        None
    }
}

/// Pick how sampling is orchestrated: `--direct` forces the in-loop read, otherwise
/// real hardware is event-driven off DRDY and the simulator is rate-paced.
const fn choose_sampling_mode(cfg: &doser_config::Config, direct: bool) -> SamplingMode {
    if direct {
        return SamplingMode::Direct;
    }
    #[cfg(all(feature = "hardware", target_os = "linux"))]
    {
        let _ = cfg; // DRDY paces the loop; the configured rate is unused
        SamplingMode::Event
    }
    #[cfg(not(all(feature = "hardware", target_os = "linux")))]
    {
        SamplingMode::Paced(cfg.filter.sample_rate_hz)
    }
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn run_dose(
    cfg: &doser_config::Config,
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
        let mode = rt_lock.unwrap_or_else(RtLock::os_default);
        setup_rt_once(rt, rt_prio, mode, rt_cpu);
    }
    #[cfg(target_os = "macos")]
    {
        let mode = rt_lock.unwrap_or_else(RtLock::os_default);
        let _ = (rt_prio, rt_cpu); // only meaningful on Linux
        setup_rt_once(rt, mode);
    }

    // Builder/config mapping — use From impls from doser_core::conversions
    let filter: doser_core::FilterCfg = (&cfg.filter).into();
    let control: doser_core::ControlCfg = (&cfg.control).into();
    let timeouts: doser_core::Timeouts = (&cfg.timeouts).into();
    let defaults = doser_core::SafetyCfg::default();
    let mut safety: doser_core::SafetyCfg = (&cfg.safety).into();
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
    let estop_check = make_estop_check(cfg);
    let sampling_mode = choose_sampling_mode(cfg, direct);
    let prefer_timeout_first = max_run_ms_override.is_none();

    // Map predictor config
    let predictor_core: doser_core::PredictorCfg = (&cfg.predictor).into();

    // Stats: control-loop latency, jitter and missed deadlines.
    //
    // The runner owns the one and only dose loop; `--stats` *observes* it rather
    // than reimplementing it, so the stats path inherits the max-run cap and the
    // sensor-stall watchdog by construction instead of by duplication.
    let mut latencies: Vec<u64> = Vec::new();
    let mut missed_deadlines: usize = 0;
    let mut sample_count: usize = 0;
    let period_us = doser_core::util::period_us(cfg.filter.sample_rate_hz);

    let params = RunParams {
        filter,
        control,
        safety,
        timeouts,
        calibration: calibration_core,
        target_g: grams,
        estop_debounce_n: cfg.estop.debounce_n,
        prefer_timeout_first,
        mode: sampling_mode,
        predictor: Some(predictor_core),
        shutdown: Some(shutdown),
    };

    let outcome = if stats {
        let mut observe = |latency: std::time::Duration| {
            let latency_us = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX);
            latencies.push(latency_us);
            if latency_us > period_us {
                missed_deadlines = missed_deadlines.saturating_add(1);
            }
            sample_count = sample_count.saturating_add(1);
        };
        doser_core::runner::run_observed(scale, motor, estop_check, params, Some(&mut observe))?
    } else {
        doser_core::runner::run(scale, motor, estop_check, params)?
    };

    if stats && !latencies.is_empty() {
        print_stats(
            &latencies,
            sample_count,
            missed_deadlines,
            cfg.filter.sample_rate_hz,
        );
    }

    let tel = JsonTelemetry {
        slope_ema_gps: outcome.slope_ema_gps,
        stop_at_g: outcome.early_stop_at_g,
        coast_comp_g: outcome.inflight_g,
    };
    Ok((outcome.final_g, tel))
}

/// Print latency/jitter stats to stderr.
//
// `cast_precision_loss` allowed: these are microsecond counters and sample counts being
// averaged for a human-readable summary; neither ever approaches 2^53.
#[allow(clippy::cast_precision_loss)]
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

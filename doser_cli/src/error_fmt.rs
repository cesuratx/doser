//! Human-readable error descriptions and structured JSON error formatting.

use crate::cli::LAST_SAFETY;
use crate::dose::abort_reason_name;

/// Map an eyre::Report to a human-readable explanation with likely causes and fix hints.
pub fn humanize(err: &eyre::Report) -> String {
    use doser_core::error::{BuildError, DoserError};

    // Typed matches first
    if let Some(be) = err.downcast_ref::<BuildError>() {
        return match be {
            BuildError::MissingScale => {
                "What happened: No scale was provided to the dosing engine.\nLikely causes: Hardware scale failed to initialize or was not wired into the builder.\nHow to fix: Ensure the HX711 scale is created successfully and passed via with_scale(...).".to_string()
            }
            BuildError::MissingMotor => {
                "What happened: No motor was provided to the dosing engine.\nLikely causes: Motor driver failed to initialize or was not wired into the builder.\nHow to fix: Ensure the motor is created successfully and passed via with_motor(...).".to_string()
            }
            BuildError::MissingTarget => {
                "What happened: Target grams not set.\nLikely causes: The CLI did not pass --grams or the builder was not configured.\nHow to fix: Provide the desired grams (e.g., `doser dose --grams 10`).".to_string()
            }
            BuildError::InvalidConfig(msg) => format!(
                "What happened: Invalid configuration ({msg}).\nLikely causes: Missing or out-of-range values in the TOML.\nHow to fix: Edit the config file, then rerun. See README for a sample."
            ),
        };
    }

    if let Some(de) = err.downcast_ref::<DoserError>() {
        // Specific domain cases first
        if matches!(de, DoserError::Timeout) {
            return "What happened: Scale read timed out.\nLikely causes: HX711 not wired correctly, no power/ground, or timeout too low.\nHow to fix: Verify DT/SCK pins and power, and consider increasing hardware.sensor_read_timeout_ms in the config.".to_string();
        }
        if let DoserError::Abort(reason) = de {
            use doser_core::error::AbortReason::*;
            return match reason {
                Estop => "What happened: Emergency stop was triggered.\nLikely causes: E-stop button pressed or input pin active.\nHow to fix: Release E-stop, ensure wiring is correct, then start a new run.".to_string(),
                NoProgress => "What happened: No progress watchdog tripped.\nLikely causes: Jammed auger, empty hopper, or scale not changing within threshold.\nHow to fix: Check mechanics and materials; adjust safety.no_progress_* in config if needed.".to_string(),
                MaxRuntime => "max run time was exceeded.\nLikely causes: Too conservative speeds, high target, or stalls.\nHow to fix: Increase safety.max_run_ms or adjust speeds/target.".to_string(),
                Overshoot => "What happened: Overshoot beyond safety limit.\nLikely causes: Inertia or too high coarse/fine speed near target.\nHow to fix: Lower speeds or increase safety.max_overshoot_g and tune epsilon/slow_at.".to_string(),
                MaxAttempts => "What happened: Internal strategy aborted after maximum attempts.\nLikely causes: Conservative settings or unexpected stall in strategy loop.\nHow to fix: Increase attempts or review control/safety settings.".to_string(),
            };
        }
        // Fallback to generic for other domain errors
        return format!(
            "What happened: {de}.\nLikely causes: See logs.\nHow to fix: Re-run with --log-level=debug or set RUST_LOG for more detail."
        );
    }

    // String-based heuristics for errors coming from init or config
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();

    if (lower.contains("hx711") && lower.contains("timeout")) || lower.contains("datareadytimeout")
    {
        return "What happened: HX711 did not produce data within the configured timeout.\nLikely causes: Wrong DT/SCK pins, wiring/power issues, or timeout configured too low.\nHow to fix: Check [pins] in the config, verify 5V/GND, and raise hardware.sensor_read_timeout_ms.".to_string();
    }

    if lower.contains("open hx711") || lower.contains("open motor pins") {
        return "What happened: Failed to initialize hardware pins.\nLikely causes: Incorrect pin numbers or insufficient GPIO permissions.\nHow to fix: Fix the [pins] values in the config; ensure the process has permission to access GPIO.".to_string();
    }

    if lower.contains("invalid configuration")
        || (lower.contains("pin") && lower.contains("missing"))
    {
        return "What happened: Configuration is invalid or incomplete.\nLikely causes: Missing [pins] (hx711_dt, hx711_sck, motor_step, motor_dir, ...), or out-of-range values.\nHow to fix: Edit the TOML config and try again.".to_string();
    }

    // Calibration CSV header special-case
    if lower.contains("calibration csv must have headers") {
        return "Invalid headers in calibration CSV. Expected 'raw,grams'.".to_string();
    }

    // Generic fallback
    let mut cause = String::new();
    if let Some(src) = err.source() {
        cause = format!(" Cause: {src}");
    }
    format!(
        "Something went wrong.{cause}\nHow to fix: Re-run with --log-level=debug for details. Original: {msg}"
    )
}

/// Map AbortReason (if present) to stable exit codes; non-abort errors return 2.
pub fn exit_code_for_error(err: &eyre::Report) -> i32 {
    use doser_core::error::DoserError;
    if let Some(DoserError::Abort(reason)) = err.downcast_ref::<DoserError>() {
        return match reason {
            doser_core::error::AbortReason::Estop => 2,
            doser_core::error::AbortReason::NoProgress => 3,
            doser_core::error::AbortReason::MaxRuntime => 4,
            doser_core::error::AbortReason::Overshoot => 5,
            doser_core::error::AbortReason::MaxAttempts => 6,
        };
    }
    1
}

/// Structured JSON for errors when --json is enabled.
pub fn format_error_json(err: &eyre::Report) -> String {
    use doser_core::error::DoserError;
    use serde_json::json;

    if let Some(DoserError::Abort(reason)) = err.downcast_ref::<DoserError>() {
        let msg = humanize(err);
        let details = LAST_SAFETY.get();
        let reason_name = abort_reason_name(reason);

        let detail_obj = match reason {
            doser_core::error::AbortReason::Overshoot => {
                details.map(|s| json!({ "max_overshoot_g": s.max_overshoot_g }))
            }
            doser_core::error::AbortReason::MaxRuntime => {
                details.map(|s| json!({ "max_run_ms": s.max_run_ms }))
            }
            doser_core::error::AbortReason::NoProgress => details.map(|s| {
                json!({ "no_progress_ms": s.no_progress_ms, "no_progress_epsilon_g": s.no_progress_epsilon_g })
            }),
            _ => None,
        };

        let obj = if let Some(d) = detail_obj {
            json!({ "reason": reason_name, "details": d, "message": msg })
        } else {
            json!({ "reason": reason_name, "message": msg })
        };
        return obj.to_string();
    }

    // Generic error JSON
    json!({ "reason": "Error", "message": humanize(err) }).to_string()
}

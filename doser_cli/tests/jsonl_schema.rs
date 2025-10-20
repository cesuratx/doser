use assert_cmd::prelude::*;
use rstest::rstest;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn write_valid_config(dir: &tempfile::TempDir) -> PathBuf {
    let toml = r#"
[pins]
# pins are unused in sim backend but must be present
hx711_dt = 5
hx711_sck = 6
motor_step = 13
motor_dir = 19
motor_en = 26
estop_in = 21

[filter]
ma_window = 1
median_window = 1
sample_rate_hz = 10

[control]
coarse_speed = 1000
fine_speed = 200
slow_at_g = 1.0
hysteresis_g = 0.05
stable_ms = 0
# epsilon must be > 0 per validation
epsilon_g = 0.02

[timeouts]
sample_ms = 10

[safety]
# Allow enough time for the throttled control loop (10 Hz) to reach 1 g in sim
max_run_ms = 5000
max_overshoot_g = 5.0
no_progress_epsilon_g = 0.02
no_progress_ms = 300  # 300ms allows for 2-3 samples at 10 Hz (100ms period)

[hardware]
sensor_read_timeout_ms = 100
"#;
    let path = dir.path().join("cfg.toml");
    fs::write(&path, toml).unwrap();
    path
}

/// Validate the JSONL schema for a successful dose run.
#[rstest]
fn jsonl_success_schema() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    // Run a small dose with JSON output
    let mut cmd = Command::cargo_bin("doser_cli").unwrap();
    cmd.arg("--json")
        .arg("--log-level")
        .arg("error")
        .arg("--config")
        .arg(&cfg)
        .arg("dose")
        .arg("--grams")
        .arg("1.0")
        // Ensure the sim progresses in case defaults change
        .env("DOSER_TEST_SIM_INC", "0.5");

    let out = cmd.assert().success().get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&out);
    let line = stdout
        .lines()
        .find(|l| l.contains("\"final_g\""))
        .unwrap_or("")
        .to_string();
    assert!(
        !line.is_empty(),
        "no JSONL line with final_g found; stdout was: {stdout}"
    );

    let v: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");

    // Required numeric fields
    assert!(v.get("timestamp").and_then(|x| x.as_i64()).is_some());
    assert!(v.get("target_g").and_then(|x| x.as_f64()).is_some());
    assert!(v.get("duration_ms").and_then(|x| x.as_u64()).is_some());

    // Either number or null
    match v.get("final_g") {
        Some(serde_json::Value::Number(n)) => assert!(n.as_f64().is_some()),
        Some(serde_json::Value::Null) => {}
        other => panic!("unexpected final_g: {other:?}"),
    }

    // Profile string
    assert!(v.get("profile").and_then(|x| x.as_str()).is_some());

    // Telemetry fields are number or null
    for key in ["slope_ema", "stop_at_g", "coast_comp_g"] {
        let ok = match v.get(key) {
            Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::Number(n)) => n.as_f64().is_some(),
            _ => false,
        };
        assert!(ok, "{key} should be number or null");
    }

    // Abort reason must be null on success
    assert!(v.get("abort_reason").is_some());
    assert!(v.get("abort_reason").unwrap().is_null());
}

/// Validate the JSONL schema for an aborted run (timeout), including abort_reason string.
#[rstest]
fn jsonl_abort_schema() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    // Force an abort by NOT setting DOSER_TEST_SIM_INC (simulator won't progress)
    // With no progress and short watchdog (50ms), will trigger NoProgress abort quickly
    let mut cmd = Command::cargo_bin("doser_cli").unwrap();
    cmd.arg("--json")
        .arg("--log-level")
        .arg("error")
        .arg("--config")
        .arg(&cfg)
        .arg("dose")
        .arg("--grams")
        .arg("10.0")
        .env("DOSER_TEST_SIM_INC", "0.0"); // No progress: will trigger NoProgress abort

    let out = cmd.assert().failure().get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&out);
    let line = stdout
        .lines()
        .find(|l| l.contains("\"final_g\""))
        .unwrap_or("")
        .to_string();
    assert!(
        !line.is_empty(),
        "no JSONL line with final_g found; stdout was: {stdout}"
    );

    let v: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");

    // Abort reason must be a non-empty string (MaxRuntime or Error)
    let abort = v.get("abort_reason").and_then(|x| x.as_str()).unwrap_or("");
    assert!(!abort.is_empty());

    // Final must be null on abort
    assert!(v.get("final_g").unwrap().is_null());
}

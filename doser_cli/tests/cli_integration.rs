use assert_cmd::prelude::*;
use predicates::prelude::*;
use rstest::rstest;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

// Build a minimal valid TOML config for sim mode
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
# valid epsilon in range
epsilon_g = 0.0

[timeouts]
sample_ms = 10

[safety]
max_run_ms = 100
max_overshoot_g = 5.0
no_progress_epsilon_g = 0.0
no_progress_ms = 0

[hardware]
sensor_read_timeout_ms = 100
"#;
    let path = dir.path().join("cfg.toml");
    fs::write(&path, toml).unwrap();
    path
}

#[rstest]
fn cli_rejects_out_of_range_epsilon() {
    let dir = tempdir().unwrap();
    let path = write_valid_config(&dir);
    // Overwrite control.epsilon_g to be invalid
    let mut invalid = fs::read_to_string(&path).unwrap();
    invalid = invalid.replace("epsilon_g = 0.0", "epsilon_g = 2.0");
    fs::write(&path, invalid).unwrap();

    let mut cmd = Command::cargo_bin("doser_cli").unwrap();
    cmd.arg("--config").arg(&path).arg("self-check");
    cmd.assert().failure().stdout(predicate::str::contains(
        "Configuration is invalid or incomplete",
    ));
}

#[rstest]
fn cli_reports_calibration_bad_header() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    // Write a bad-header CSV
    let bad_csv = dir.path().join("calib.csv");
    let mut f = fs::File::create(&bad_csv).unwrap();
    writeln!(f, "raw,value").unwrap();
    writeln!(f, "100,0.0").unwrap();
    writeln!(f, "200,1.0").unwrap();

    let mut cmd = Command::cargo_bin("doser_cli").unwrap();
    cmd.arg("--config")
        .arg(&cfg)
        .arg("--calibration")
        .arg(&bad_csv)
        .arg("self-check");
    cmd.assert()
        .failure()
        .stdout(predicate::str::contains("headers 'raw,grams'"));
}

use assert_cmd::prelude::*;
use predicates::prelude::*;
use rstest::rstest;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[rstest]
fn hx711_timeout_bubbles_to_cli() {
    let dir = tempdir().unwrap();
    let toml = r#"
[pins]
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
epsilon_g = 0.0

[timeouts]
sample_ms = 5

[safety]
max_run_ms = 50
max_overshoot_g = 5.0
no_progress_epsilon_g = 0.02
no_progress_ms = 1200

[hardware]
sensor_read_timeout_ms = 50
"#;
    let cfg = dir.path().join("cfg.toml");
    fs::write(&cfg, toml).unwrap();

    let mut cmd = Command::cargo_bin("doser_cli").unwrap();
    cmd.env("DOSER_TEST_SIM_TIMEOUT", "1");
    cmd.arg("--config")
        .arg(&cfg)
        .arg("dose")
        .arg("--grams")
        .arg("0.5");
    cmd.assert().failure().stdout(predicate::str::contains(
        "What happened: Scale read timed out",
    ));
}

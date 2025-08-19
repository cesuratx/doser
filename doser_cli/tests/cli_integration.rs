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
# epsilon must be > 0 per validation
epsilon_g = 0.02

[timeouts]
sample_ms = 10

[safety]
# Allow enough time for the throttled control loop (10 Hz) to reach 5 g in sim
max_run_ms = 5000
max_overshoot_g = 5.0
no_progress_epsilon_g = 0.02
no_progress_ms = 1200

[hardware]
sensor_read_timeout_ms = 100
"#;
    let path = dir.path().join("cfg.toml");
    fs::write(&path, toml).unwrap();
    path
}

#[rstest]
#[case(&["--help"], 0, "Usage:", "stdout")]
#[case(&["dose", "--grams", "5"], 0, "complete", "stdout")]
#[case(&["dose"], 2, "required", "stderr")]
#[case(&["dose", "--grams", "5", "--max-run-ms", "1"], -1, "max run time", "stderr")]
fn cli_table_cases(
    #[case] args: &[&str],
    #[case] exit_code: i32,
    #[case] needle: &str,
    #[case] stream: &str,
) {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    let mut cmd = Command::cargo_bin("doser_cli").unwrap();

    // Always include a valid config to avoid relying on default path
    cmd.arg("--config").arg(&cfg);

    // For dose runs that should progress, nudge the sim scale to increase
    if args.first().copied() == Some("dose") && exit_code == 0 {
        cmd.env("DOSER_TEST_SIM_INC", "0.5");
    }

    for a in args {
        cmd.arg(a);
    }

    let assert = cmd.assert();

    // Check exit status in a chained manner to keep ownership
    let assert = if exit_code >= 0 {
        assert.code(exit_code)
    } else {
        assert.failure()
    };

    match stream {
        "stdout" => {
            assert.stdout(predicate::str::contains(needle));
        }
        "stderr" => {
            assert.stderr(predicate::str::contains(needle));
        }
        other => panic!("unknown stream: {other}"),
    }
}

#[rstest]
fn cli_reports_bad_calibration_header() {
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
        .stderr(predicate::str::contains("Invalid headers"));
}

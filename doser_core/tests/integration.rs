//! Integration tests aligned with the new CLI + core API.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use doser_core::{ControlCfg, Doser, FilterCfg, Timeouts};
use doser_traits::{Motor, Scale};
use rstest::rstest;
use std::fs;

fn bin_path() -> PathBuf {
    // 1) Prefer Cargo’s runtime var if present (some setups export it)
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_doser_cli") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }

    // 2) Otherwise, compute from the workspace `target/<profile>/doser_cli`
    //    We are executing from the `doser_core` crate, so go up to workspace root.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // .../doser/doser_core
    let workspace_root = manifest_dir.parent().expect("workspace root");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string()); // default to debug
    let exe_name = if cfg!(windows) {
        "doser_cli.exe"
    } else {
        "doser_cli"
    };
    let candidate = workspace_root.join("target").join(profile).join(exe_name);
    candidate
}

fn write_temp_config() -> PathBuf {
    let path = std::env::temp_dir().join(format!("doser_test_cfg_{}.toml", std::process::id()));
    let toml = r#"
        [pins]
        hx711_dt = 5
        hx711_sck = 6
        motor_step = 13
        motor_dir = 19
        estop_in = 26

        [filter]
        ma_window = 1
        median_window = 1
        sample_rate_hz = 50

        [control]
        coarse_speed = 1200
        fine_speed = 250
        slow_at_g = 1.0
        hysteresis_g = 0.05
        stable_ms = 250

        [timeouts]
        sample_ms = 100

        [logging]
        level = "info"
    "#;
    fs::write(&path, toml).expect("write temp config");
    path
}

fn ensure_exists(p: &Path) {
    if !p.exists() {
        panic!(
            "CLI binary not found at {:?}.\n\
             Make sure it’s built: `cargo build -p doser_cli`\n\
             (This test looks under <workspace>/target/<profile>/doser_cli.)",
            p
        );
    }
}

#[rstest]
fn cli_missing_arguments_prints_help() {
    let exe = bin_path();
    ensure_exists(&exe);

    let out = Command::new(&exe).output().expect("spawn doser_cli");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    let help_text = format!("{}\n{}", stdout, stderr);
    assert!(
        help_text.contains("Usage:") && help_text.contains("doser_cli"),
        "Expected help message; got:\nSTDOUT:\n{}\n\nSTDERR:\n{}\n",
        stdout,
        stderr
    );
}

#[rstest]
#[case(false)]
#[case(true)]
fn cli_self_check_prints_ok(#[case] json: bool) {
    // With no `hardware` feature, the CLI uses simulated scale/motor.
    let exe = bin_path();
    ensure_exists(&exe);
    let cfg = write_temp_config();

    let mut cmd = Command::new(&exe);
    cmd.arg("--config").arg(cfg.as_os_str());
    if json {
        cmd.arg("--json");
    }
    cmd.arg("self-check");

    let out = cmd.output().expect("spawn doser_cli self-check");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "CLI exited with non-zero status.\nSTDOUT:\n{}\n\nSTDERR:\n{}\n",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("OK"),
        "Expected OK from self-check; got:\n{}\n",
        stdout
    );
}

/// A scale that always errors — used to ensure core maps errors properly.
struct ErrScale;
impl Scale for ErrScale {
    fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        Err("simulated sensor error".into())
    }
}

#[derive(Default)]
struct NopMotor;
impl Motor for NopMotor {
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

#[rstest]
fn simulated_hardware_error_in_core() {
    let mut doser = Doser::builder()
        .with_scale(ErrScale)
        .with_motor(NopMotor::default())
        .with_filter(FilterCfg::default())
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 5 })
        .with_target_grams(5.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap_or_else(|e| panic!("build should succeed: {e}"));

    let err = doser
        .step()
        .expect_err("step should fail due to scale error");

    let de = err
        .downcast_ref::<doser_core::error::DoserError>()
        .expect("expected DoserError inside Report");
    match de {
        doser_core::error::DoserError::Hardware(s) => {
            assert!(s.contains("sensor error") || !s.is_empty())
        }
        doser_core::error::DoserError::HardwareFault(s) => assert!(!s.is_empty()),
        other => panic!("expected hardware-mapped error, got: {other:?}"),
    }
}

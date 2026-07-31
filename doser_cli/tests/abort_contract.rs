//! The two machine-readable failure contracts: the process exit code
//! (`error_fmt::exit_code_for_error`) and, under `--json`, the structured error
//! object (`error_fmt::format_error_json`).
//!
//! Both are consumed by operator scripts and by the systemd unit, so every code
//! and every `details` key is pinned with an exact assertion. Each abort is
//! driven deterministically from config/flags plus the simulator's
//! `DOSER_TEST_SIM_INC` / `DOSER_TEST_SIM_TIMEOUT` hooks — never from timing luck.
//!
//! Known wart, recorded rather than papered over: `AbortReason::Estop` maps to
//! exit 2, which is also clap's usage-error code (`common::exit::USAGE`). A
//! caller that only sees the exit status cannot tell "the E-stop tripped" from
//! "you mistyped the command". See `common::exit::ESTOP`.

mod common;

use assert_cmd::prelude::*;
use common::{Cfg, doser, exit};
use predicates::prelude::*;
use rstest::rstest;
use serde_json::Value;
use tempfile::tempdir;

/// The error object is the LAST line of stdout in `--json` mode (the dose result
/// line comes first). Parse it rather than grepping so a malformed object fails.
fn error_json(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let last = text
        .lines()
        .next_back()
        .unwrap_or_else(|| panic!("no stdout at all; expected a JSON error object"));
    serde_json::from_str(last).unwrap_or_else(|e| panic!("error line is not JSON ({e}): {last:?}"))
}

// ── Exit codes ───────────────────────────────────────────────────────────────

/// No progress: the sim scale never moves, so the watchdog trips first.
/// `no_progress_ms` (200) is well under `max_run_ms` (4000), so the max-run cap
/// cannot win the race.
#[rstest]
fn no_progress_aborts_with_exit_3() {
    let dir = tempdir().unwrap();
    let cfg = Cfg {
        no_progress_ms: 200,
        ..Cfg::default()
    }
    .write(&dir);

    doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.0")
        .args(["dose", "--grams", "10"])
        .assert()
        .code(exit::NO_PROGRESS)
        .stderr(predicate::str::contains(
            "What happened: No progress watchdog tripped.",
        ));
}

/// Max runtime: `--max-run-ms 50` also flips the runner's precedence so the
/// max-run cap is evaluated before the sensor-stall check, making this the only
/// abort that can fire. The sim never progresses, so the run cannot complete.
#[rstest]
fn max_runtime_aborts_with_exit_4() {
    let dir = tempdir().unwrap();
    let cfg = Cfg::default().write(&dir);

    doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.0")
        .args(["dose", "--grams", "10", "--max-run-ms", "50"])
        .assert()
        .code(exit::MAX_RUNTIME)
        .stderr(predicate::str::contains("max run time was exceeded."));
}

/// Overshoot: a 5 g-per-read simulator against a 1 g target with a 0.5 g
/// overshoot budget. The first sample taken after the motor starts is already
/// 5 g, and `process_weight` checks the overshoot guard before the completion
/// zone, so the run can only end in `Overshoot`.
#[rstest]
fn overshoot_aborts_with_exit_5() {
    let dir = tempdir().unwrap();
    let cfg = Cfg::default().write(&dir);

    doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "5.0")
        .args(["dose", "--grams", "1", "--max-overshoot-g", "0.5"])
        .assert()
        .code(exit::OVERSHOOT)
        .stderr(predicate::str::contains(
            "What happened: Overshoot beyond safety limit.",
        ));
}

/// A scale read timeout is *not* an abort: it is `DoserError::Timeout`, which
/// falls through to the generic exit 1.
#[rstest]
fn scale_timeout_is_not_an_abort_and_exits_1() {
    let dir = tempdir().unwrap();
    let cfg = Cfg {
        sample_rate_hz: 10,
        sample_ms: 5,
        sensor_read_timeout_ms: 50,
        max_run_ms: 5000,
        epsilon_g: 0.02,
        ..Cfg::default()
    }
    .write(&dir);

    doser(&cfg)
        .env("DOSER_TEST_SIM_TIMEOUT", "1")
        .args(["dose", "--grams", "0.5"])
        .assert()
        .code(exit::OTHER)
        .stderr(predicate::str::contains(
            "What happened: Scale read timed out.",
        ));
}

/// A shutdown signal is reported as `AbortReason::Estop`, exit 2. Driven with a
/// real SIGINT because that is the only way the flag is ever set in production
/// (the ctrl-c handler).
///
/// Signalling on a fixed delay would be a race: a SIGINT delivered before
/// `ctrlc::set_handler` runs kills the process outright. Instead the test waits
/// for the run's own `dose start` record to appear in a log file, which can only
/// be written after the handler is installed — so the signal provably lands with
/// the handler in place and the dose loop running. The watchdogs are set to 10 s
/// and 20 s, far longer than the wait, so the run cannot end on its own first.
#[cfg(unix)]
#[rstest]
fn shutdown_signal_aborts_as_estop_with_exit_2() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let dir = tempdir().unwrap();
    let log = dir.path().join("dose.log");
    let cfg = Cfg {
        max_run_ms: 20_000,
        no_progress_ms: 10_000,
        extra: format!("\n[logging]\nfile = {:?}\nrotation = \"never\"\n", log),
        ..Cfg::default()
    }
    .write(&dir);

    let child = doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.0")
        .args(["dose", "--grams", "10"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn doser_cli");

    // Readiness gate: `dose start` is logged after tracing (and therefore the
    // ctrl-c handler, installed earlier still) is up and the loop has begun.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if std::fs::read_to_string(&log).is_ok_and(|s| s.contains("dose start")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "doser_cli never reached `dose start`"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let status = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");
    assert!(status.success(), "kill -INT failed");

    let out = child.wait_with_output().expect("wait for doser_cli");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit::ESTOP),
        "expected the E-stop exit code; stderr was: {stderr}"
    );
    assert!(
        stderr.contains("Received shutdown signal"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("What happened: Emergency stop was triggered."),
        "stderr was: {stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("final:"),
        "an interrupted dose must not report a final weight"
    );
}

/// The usage-error code, asserted next to the abort codes so the collision with
/// `Estop` (both 2) is visible in one place.
#[rstest]
fn clap_usage_error_shares_exit_2_with_estop() {
    let dir = tempdir().unwrap();
    let cfg = Cfg::default().write(&dir);

    doser(&cfg).arg("dose").assert().code(exit::USAGE);
    assert_eq!(
        exit::USAGE,
        exit::ESTOP,
        "if these ever diverge, update the note in this module's docs"
    );
}

// ── --json error schema ──────────────────────────────────────────────────────

/// `NoProgress` carries both no-progress knobs, taken from the *effective*
/// safety config (here: straight from the TOML, since neither has a CLI flag).
#[rstest]
fn json_error_details_for_no_progress() {
    let dir = tempdir().unwrap();
    let cfg = Cfg {
        no_progress_ms: 250,
        no_progress_epsilon_g: 0.03,
        ..Cfg::default()
    }
    .write(&dir);

    let out = doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.0")
        .args(["--json", "--log-level", "error", "dose", "--grams", "10"])
        .assert()
        .code(exit::NO_PROGRESS)
        .get_output()
        .clone();

    let v = error_json(&out.stdout);
    assert_eq!(v["reason"], "NoProgress");
    assert!(
        v["message"]
            .as_str()
            .is_some_and(|m| m.contains("No progress watchdog tripped")),
        "message was {:?}",
        v["message"]
    );
    assert_eq!(v["details"]["no_progress_ms"], 250);
    // f32 -> f64 widening: 0.03f32 is not exactly 0.03 in f64.
    let eps = v["details"]["no_progress_epsilon_g"]
        .as_f64()
        .expect("no_progress_epsilon_g is a number");
    assert!((eps - 0.03).abs() < 1e-6, "epsilon was {eps}");
}

/// `MaxRuntime` carries `max_run_ms`, and it must be the value that actually
/// governed the run: the CLI override, not the (very different) config value.
/// This is the assertion that proves LAST_SAFETY is populated *after* the
/// overrides are merged.
#[rstest]
fn json_error_details_reflect_the_max_run_ms_override_not_the_config() {
    let dir = tempdir().unwrap();
    let cfg = Cfg {
        max_run_ms: 4000,
        ..Cfg::default()
    }
    .write(&dir);

    let out = doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.0")
        .args([
            "--json",
            "--log-level",
            "error",
            "dose",
            "--grams",
            "10",
            "--max-run-ms",
            "60",
        ])
        .assert()
        .code(exit::MAX_RUNTIME)
        .get_output()
        .clone();

    let v = error_json(&out.stdout);
    assert_eq!(v["reason"], "MaxRuntime");
    assert_eq!(
        v["details"]["max_run_ms"], 60,
        "details must report the effective (overridden) value, not config's 4000"
    );
    assert!(
        v["message"]
            .as_str()
            .is_some_and(|m| m.contains("max run time was exceeded")),
        "message was {:?}",
        v["message"]
    );
}

/// `Overshoot` carries `max_overshoot_g`, again the effective value: config says
/// 5.0 g, the flag says 0.5 g, and 0.5 is what both the abort and the payload
/// must reflect.
#[rstest]
fn json_error_details_reflect_the_max_overshoot_override() {
    let dir = tempdir().unwrap();
    let cfg = Cfg {
        max_overshoot_g: 5.0,
        ..Cfg::default()
    }
    .write(&dir);

    let out = doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "5.0")
        .args([
            "--json",
            "--log-level",
            "error",
            "dose",
            "--grams",
            "1",
            "--max-overshoot-g",
            "0.5",
        ])
        .assert()
        .code(exit::OVERSHOOT)
        .get_output()
        .clone();

    let v = error_json(&out.stdout);
    assert_eq!(v["reason"], "Overshoot");
    let max_overshoot = v["details"]["max_overshoot_g"]
        .as_f64()
        .expect("max_overshoot_g is a number");
    assert!(
        (max_overshoot - 0.5).abs() < 1e-6,
        "details must report the effective 0.5 g, got {max_overshoot}"
    );
}

/// Non-abort failures get the generic object: `reason: "Error"` and a message,
/// with no `details` key at all.
#[rstest]
fn json_error_object_for_a_non_abort_failure_has_no_details() {
    let dir = tempdir().unwrap();
    let cfg = Cfg {
        sample_rate_hz: 10,
        sample_ms: 5,
        sensor_read_timeout_ms: 50,
        max_run_ms: 5000,
        ..Cfg::default()
    }
    .write(&dir);

    let out = doser(&cfg)
        .env("DOSER_TEST_SIM_TIMEOUT", "1")
        .args(["--json", "--log-level", "error", "dose", "--grams", "0.5"])
        .assert()
        .code(exit::OTHER)
        .get_output()
        .clone();

    let v = error_json(&out.stdout);
    assert_eq!(v["reason"], "Error");
    assert!(v.get("details").is_none(), "details must be absent: {v}");
    assert!(
        v["message"]
            .as_str()
            .is_some_and(|m| m.contains("Scale read timed out")),
        "message was {:?}",
        v["message"]
    );
}

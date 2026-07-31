//! The `--json` stdout contract.
//!
//! In `--json` mode stdout is a machine-readable stream and *nothing else*: one
//! JSONL result line for a completed dose, or (on failure) the result line
//! followed by the structured error object. Every tracing record belongs on
//! stderr. These tests parse whole lines rather than grepping for a field name,
//! because a JSON *log* record carrying `final_g` is indistinguishable from the
//! real result line to anything that greps — which is exactly how log-on-stdout
//! went unnoticed.

mod common;

use assert_cmd::prelude::*;
use common::{Cfg, doser, exit};
use rstest::rstest;
use serde_json::Value;
use tempfile::tempdir;

/// The JSON-mode config: 10 Hz so the run is short but still multi-sample.
fn json_cfg() -> Cfg {
    Cfg {
        sample_rate_hz: 10,
        sample_ms: 10,
        max_run_ms: 5000,
        // 300 ms allows for 2-3 samples at 10 Hz (100 ms period)
        no_progress_ms: 300,
        ..Cfg::default()
    }
}

/// Split stdout into lines, requiring each to be a standalone JSON object.
fn json_lines(stdout: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(stdout);
    text.lines()
        .map(|l| {
            serde_json::from_str::<Value>(l)
                .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {l:?}"))
        })
        .collect()
}

fn assert_result_line_shape(v: &Value, target_g: f64) {
    assert!(v.get("timestamp").and_then(Value::as_i64).is_some());
    assert_eq!(v["target_g"].as_f64(), Some(target_g));
    assert!(v.get("duration_ms").and_then(Value::as_u64).is_some());
    assert!(v.get("profile").and_then(Value::as_str).is_some());
    for key in ["slope_ema", "stop_at_g", "coast_comp_g"] {
        let ok = matches!(v.get(key), Some(Value::Null) | Some(Value::Number(_)));
        assert!(ok, "{key} should be number or null, got {:?}", v.get(key));
    }
}

/// A successful `--json` dose writes exactly ONE line to stdout, and it is the
/// result object. Anything else on stdout — a log record, a stray println — is
/// a contract break and fails here.
#[rstest]
fn jsonl_success_schema_is_exactly_one_line() {
    let dir = tempdir().unwrap();
    let cfg = json_cfg().write(&dir);

    let out = doser(&cfg)
        .args(["--json", "--log-level", "error", "dose", "--grams", "1.0"])
        .env("DOSER_TEST_SIM_INC", "0.5")
        .assert()
        .success()
        .get_output()
        .clone();

    let lines = json_lines(&out.stdout);
    assert_eq!(
        lines.len(),
        1,
        "stdout must be exactly one JSON line; got {}: {:?}",
        lines.len(),
        String::from_utf8_lossy(&out.stdout)
    );

    let v = &lines[0];
    assert_result_line_shape(v, 1.0);
    assert!(
        v["final_g"].as_f64().is_some_and(|g| g >= 1.0),
        "final_g must be a number at or past the target, got {:?}",
        v["final_g"]
    );
    assert!(v["abort_reason"].is_null(), "abort_reason must be null");
}

/// Regression test for log records leaking onto stdout.
///
/// `RUST_LOG` overrides `--log-level` (see `tracing_setup::init_tracing`), so
/// this run emits INFO records regardless of the flag — and in `--json` mode
/// those records are themselves JSON objects with a `final_g` field. If the
/// console layer ever loses its `stderr` writer, stdout gains extra lines and
/// this test fails. The old `--log-level error` + "find the line containing
/// final_g" approach could not see that.
#[rstest]
fn jsonl_stdout_stays_one_line_under_rust_log_info() {
    let dir = tempdir().unwrap();
    let cfg = json_cfg().write(&dir);

    let out = doser(&cfg)
        .args(["--json", "--log-level", "error", "dose", "--grams", "1.0"])
        .env("DOSER_TEST_SIM_INC", "0.5")
        .env("RUST_LOG", "info")
        .assert()
        .success()
        .get_output()
        .clone();

    let lines = json_lines(&out.stdout);
    assert_eq!(
        lines.len(),
        1,
        "log records must not reach stdout; stdout was: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_result_line_shape(&lines[0], 1.0);
    assert!(lines[0]["abort_reason"].is_null());

    // Proof the INFO records were actually emitted (otherwise the test would
    // pass vacuously if RUST_LOG stopped taking effect) — on stderr.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dose start") && stderr.contains("dose complete"),
        "expected INFO records on stderr; stderr was: {stderr}"
    );
}

/// An aborted `--json` dose writes exactly TWO lines: the result line (with
/// `abort_reason` set and `final_g` null) and then the structured error object.
/// Both are pinned so neither can quietly grow a third line or lose one.
#[rstest]
fn jsonl_abort_schema_is_result_line_then_error_object() {
    let dir = tempdir().unwrap();
    let cfg = json_cfg().write(&dir);

    // No sim increment: the scale never moves, so the no-progress watchdog trips.
    let out = doser(&cfg)
        .args(["--json", "--log-level", "error", "dose", "--grams", "10.0"])
        .env("DOSER_TEST_SIM_INC", "0.0")
        .assert()
        .code(exit::NO_PROGRESS)
        .get_output()
        .clone();

    let lines = json_lines(&out.stdout);
    assert_eq!(
        lines.len(),
        2,
        "expected result line + error object; stdout was: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    let result = &lines[0];
    assert_result_line_shape(result, 10.0);
    assert!(result["final_g"].is_null(), "final_g must be null on abort");
    assert_eq!(result["abort_reason"], "NoProgress");

    let error = &lines[1];
    assert_eq!(error["reason"], "NoProgress");
    assert!(error.get("message").and_then(Value::as_str).is_some());
}

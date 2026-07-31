//! End-to-end behaviour of the `dose` happy path and the top-level argument
//! surface, asserted against the binary's *user-facing* contract.
//!
//! stdout carries only the CLI's own output (`final: X.XX g`, or the `--json`
//! result line); every tracing record goes to stderr. Tests here must never pin
//! a log string on stdout — doing so is what let log records leak onto stdout
//! unnoticed in the first place.

mod common;

use assert_cmd::prelude::*;
use common::{Cfg, doser, exit, write_valid_config};
use predicates::prelude::*;
use rstest::rstest;
use tempfile::tempdir;

#[rstest]
#[case(&["--help"], 0, "Usage:", "stdout")]
// The user-facing success output is `final: X.XX g` on stdout. This case used to
// assert "complete", which only ever matched the `dose complete` *log* record.
#[case(&["dose", "--grams", "5"], 0, "final:", "stdout")]
// Companion case pinning the log stream: the record exists, on stderr.
#[case(&["dose", "--grams", "5"], 0, "dose complete", "stderr")]
#[case(&["dose"], exit::USAGE, "required", "stderr")]
fn cli_table_cases(
    #[case] args: &[&str],
    #[case] exit_code: i32,
    #[case] needle: &str,
    #[case] stream: &str,
) {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    let mut cmd = doser(&cfg);

    // For dose runs that should progress, nudge the sim scale to increase
    if args.first().copied() == Some("dose") && exit_code == 0 {
        cmd.env("DOSER_TEST_SIM_INC", "0.5");
    }

    for a in args {
        cmd.arg(a);
    }

    let assert = cmd.assert().code(exit_code);

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

/// The full stdout contract for a successful non-JSON dose: exactly one line,
/// `final: <grams> g`, and nothing else. The grams value is parsed rather than
/// string-matched because the sim lands on the first sample at or past the
/// target, which depends on how many 0.5 g increments the sampler coalesces.
#[rstest]
fn dose_stdout_is_only_the_final_line() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    let out = doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.5")
        .args(["dose", "--grams", "2"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout should be one line, got: {stdout:?}");

    let grams = lines[0]
        .strip_prefix("final: ")
        .and_then(|r| r.strip_suffix(" g"))
        .unwrap_or_else(|| panic!("stdout line is not `final: X.XX g`: {:?}", lines[0]))
        .parse::<f32>()
        .expect("final grams parses as a number");
    assert!(
        grams >= 2.0,
        "a completed dose must reach the target, got {grams}"
    );

    // The dose log records exist — on stderr, where the contract puts them.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("dose start"), "stderr was: {stderr}");
    assert!(stderr.contains("dose complete"), "stderr was: {stderr}");
}

/// `--stats` is not just formatting: it routes the dose through the runner's
/// observer hook. Assert both that the run still succeeds and that the stats
/// block lands on stderr (stdout stays reserved for the result).
#[rstest]
fn dose_stats_block_goes_to_stderr_and_the_dose_still_succeeds() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.5")
        .args(["dose", "--grams", "1", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("final:"))
        .stdout(predicate::str::contains("Doser Stats").not())
        .stderr(predicate::str::contains("--- Doser Stats ---"))
        .stderr(predicate::str::contains("Samples: "))
        .stderr(predicate::str::contains("Period (us): "))
        .stderr(predicate::str::contains("Latency min/avg/max/stdev (us): "))
        .stderr(predicate::str::contains("Missed deadlines (> period): "));
}

/// `--print-runtime` reports on stderr, so it cannot corrupt a stdout consumer.
#[rstest]
fn dose_print_runtime_reports_on_stderr() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    let out = doser(&cfg)
        .env("DOSER_TEST_SIM_INC", "0.5")
        .args(["dose", "--grams", "1", "--print-runtime"])
        .assert()
        .success()
        .get_output()
        .clone();

    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("runtime:"),
        "runtime line must not be on stdout"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr
        .lines()
        .find(|l| l.trim_start().starts_with("runtime: "))
        .unwrap_or_else(|| panic!("no `runtime: N ms` line on stderr; stderr was: {stderr}"));
    let ms = line
        .trim_start()
        .trim_start_matches("runtime: ")
        .trim_end_matches(" ms")
        .parse::<u64>()
        .expect("runtime is a whole number of ms");
    assert!(ms < 60_000, "implausible runtime {ms} ms");
}

/// `health` exercises both backends and reports each with a ✓ line.
#[rstest]
fn health_reports_both_subsystems_ok_on_the_sim_backend() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    doser(&cfg)
        .arg("health")
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Scale: responsive (raw: "))
        .stdout(predicate::str::contains("✓ Motor: responsive"))
        .stdout(predicate::str::contains("Health check: OK"));
}

/// `self-check` classifies the sensor rate and prints it on stdout.
#[rstest]
fn cli_self_check_reports_sps() {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    let out = doser(&cfg)
        .arg("self-check")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("Detected HX711 rate: "))
        .unwrap_or_else(|| panic!("no rate line; stdout was: {stdout}"));
    // Only two classifications exist (<50 ms median => 80 SPS, else 10 SPS).
    assert!(
        line == "Detected HX711 rate: 80 SPS" || line == "Detected HX711 rate: 10 SPS",
        "unexpected rate line: {line}"
    );
}

/// A dose that never sees the scale move must not report success, and the
/// override-free path must still be bounded by the config's watchdogs.
#[rstest]
fn dose_without_progress_fails_and_prints_no_final_line() {
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
        .stdout(predicate::str::contains("final:").not());
}

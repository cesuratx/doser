//! The `motor` jog subcommand — the bench bring-up tool.
//!
//! Its contract is the two status lines on stdout and the argument validation
//! clap performs before the motor is ever energised. The `--steps` semantics are
//! deliberately pinned here: the flag is *duration-derived* sugar (N/--sps
//! seconds, rounded up, at the clamped rate), and the printed duration is the
//! only externally visible proof of that conversion.

mod common;

use assert_cmd::prelude::*;
use common::{doser, exit, write_valid_config};
use predicates::prelude::*;
use rstest::rstest;
use tempfile::tempdir;

/// Run a jog and return its stdout lines.
fn jog(args: &[&str]) -> Vec<String> {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);
    let out = doser(&cfg)
        .arg("motor")
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8_lossy(&out)
        .lines()
        .map(str::to_string)
        .collect()
}

#[rstest]
#[case(&["--sps", "200", "--ms", "50"], "motor jog: 200 sps cw for 50 ms")]
#[case(&["--sps", "200", "--ms", "50", "--dir", "cw"], "motor jog: 200 sps cw for 50 ms")]
#[case(&["--sps", "200", "--ms", "50", "--dir", "ccw"], "motor jog: 200 sps ccw for 50 ms")]
fn jog_prints_its_parameters_then_stops(#[case] args: &[&str], #[case] expected_first: &str) {
    let lines = jog(args);
    assert_eq!(lines.len(), 2, "unexpected stdout: {lines:?}");
    assert!(
        lines[0].starts_with(expected_first),
        "first line was {:?}",
        lines[0]
    );
    assert!(
        lines[0].contains("press Ctrl-C to stop early"),
        "first line was {:?}",
        lines[0]
    );
    assert_eq!(lines[1], "motor jog: done — stopped");
}

/// `--steps` overrides `--ms` and is converted to a duration of N/--sps seconds,
/// rounded **up** (100 steps at 300 sps is 334 ms, not the truncated 333).
#[rstest]
#[case(&["--sps", "200", "--ms", "9999", "--steps", "20"], "for 100 ms")]
#[case(&["--sps", "300", "--ms", "9999", "--steps", "100"], "for 334 ms")]
fn steps_override_ms_and_round_up(#[case] args: &[&str], #[case] expected: &str) {
    let lines = jog(args);
    assert!(
        lines[0].contains(expected),
        "expected {expected:?} in {:?}",
        lines[0]
    );
}

/// The rate is range-checked by clap against the driver's ceiling, so an
/// impossible rate is refused before anything is driven — the commanded rate and
/// the stepped rate can never disagree.
#[rstest]
#[case("20000")]
#[case("5001")]
#[case("0")]
fn out_of_range_step_rates_are_refused(#[case] sps: &str) {
    let dir = tempdir().unwrap();
    let cfg = write_valid_config(&dir);

    doser(&cfg)
        .args(["motor", "--sps", sps, "--ms", "10"])
        .assert()
        .code(exit::USAGE)
        .stderr(predicate::str::contains("is not in 1..=5000"))
        .stdout(predicate::str::contains("motor jog").not());
}

/// The ceiling itself is accepted — the bound is inclusive.
#[rstest]
fn the_maximum_step_rate_is_accepted() {
    let lines = jog(&["--sps", "5000", "--ms", "20"]);
    assert!(
        lines[0].starts_with("motor jog: 5000 sps cw for 20 ms"),
        "first line was {:?}",
        lines[0]
    );
}

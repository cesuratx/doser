//! Startup failure modes: the config file and the calibration CSV.
//!
//! These all run before any hardware is touched, so they are pure input
//! validation — and they are the errors an operator is most likely to hit. Each
//! test pins the exit code and the operator-facing text, because `humanize()`
//! routes different failures down visibly different branches and a silent
//! reroute (e.g. a config error falling through to the generic branch) is a real
//! regression in the message quality.

mod common;

use assert_cmd::prelude::*;
use common::{Cfg, doser, exit, write_named};
use predicates::prelude::*;
use rstest::rstest;
use tempfile::tempdir;

// ── Config file ──────────────────────────────────────────────────────────────

#[rstest]
fn missing_config_file_is_reported_with_its_cause() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("does-not-exist.toml");

    doser(&missing)
        .args(["dose", "--grams", "1"])
        .assert()
        .code(exit::OTHER)
        .stderr(predicate::str::contains("Something went wrong."))
        .stderr(predicate::str::contains("No such file or directory"));
}

#[rstest]
fn invalid_toml_is_reported_as_a_parse_failure() {
    let dir = tempdir().unwrap();
    let cfg = write_named(&dir, "cfg.toml", "[pins\nthis is not toml = = 3\n");

    doser(&cfg)
        .args(["dose", "--grams", "1"])
        .assert()
        .code(exit::OTHER)
        .stderr(predicate::str::contains("parse config"));
}

/// A structurally valid TOML that `Config::validate()` rejects must land in
/// humanize()'s dedicated configuration branch, not the generic fallback.
#[rstest]
fn validation_failure_uses_the_configuration_branch() {
    let dir = tempdir().unwrap();
    // coarse_speed = 0 is rejected by validate() ("control.coarse_speed must be > 0").
    let cfg = Cfg {
        coarse_speed: 0,
        ..Cfg::default()
    }
    .write(&dir);

    doser(&cfg)
        .args(["dose", "--grams", "1"])
        .assert()
        .code(exit::OTHER)
        .stderr(predicate::str::contains(
            "What happened: Configuration is invalid or incomplete.",
        ));
}

/// The 1 MiB `MAX_CONFIG_BYTES` guard in `main.rs`: the file is refused on size
/// alone, before it is read into memory or parsed. The padding is TOML comment
/// text, so the file would otherwise be perfectly valid — proving the size check
/// is what rejected it.
#[rstest]
fn oversized_config_is_refused_before_parsing() {
    const MAX_CONFIG_BYTES: usize = 1 << 20;

    let dir = tempdir().unwrap();
    let mut text = Cfg::default().to_toml();
    text.push('\n');
    // One long comment line pushes the file just past the cap.
    text.push_str("# ");
    let pad = MAX_CONFIG_BYTES + 1 - text.len();
    text.extend(std::iter::repeat_n('x', pad));
    text.push('\n');
    assert!(text.len() > MAX_CONFIG_BYTES);
    let cfg = write_named(&dir, "huge.toml", &text);

    doser(&cfg)
        .args(["dose", "--grams", "1"])
        .assert()
        .code(exit::OTHER)
        .stderr(predicate::str::contains("is too large"))
        .stderr(predicate::str::contains("1048576 byte limit"));
}

/// The same content just *under* the cap is accepted, so the test above is
/// pinning the boundary rather than "big files break somehow".
#[rstest]
fn config_just_under_the_size_cap_is_accepted() {
    const MAX_CONFIG_BYTES: usize = 1 << 20;

    let dir = tempdir().unwrap();
    let mut text = Cfg::default().to_toml();
    text.push_str("\n# ");
    let pad = MAX_CONFIG_BYTES - text.len() - 1;
    text.extend(std::iter::repeat_n('x', pad));
    text.push('\n');
    assert_eq!(text.len(), MAX_CONFIG_BYTES);
    let cfg = write_named(&dir, "big.toml", &text);

    doser(&cfg).arg("self-check").assert().success();
}

// ── Calibration CSV ──────────────────────────────────────────────────────────

/// Load the calibration through `self-check`, which parses it and then exits
/// without needing the control loop.
fn run_with_calibration(csv_body: &str) -> assert_cmd::assert::Assert {
    let dir = tempdir().unwrap();
    let cfg = Cfg::default().write(&dir);
    let csv = write_named(&dir, "calib.csv", csv_body);
    doser(&cfg)
        .arg("--calibration")
        .arg(&csv)
        .arg("self-check")
        .assert()
}

#[rstest]
fn calibration_with_wrong_headers_is_rejected() {
    run_with_calibration("raw,value\n100,0.0\n200,1.0\n")
        .code(exit::OTHER)
        .stderr(predicate::str::contains(
            "Invalid headers in calibration CSV. Expected 'raw,grams'.",
        ));
}

#[rstest]
fn calibration_with_a_single_row_is_rejected() {
    run_with_calibration("raw,grams\n100,0.0\n")
        .code(exit::OTHER)
        .stderr(predicate::str::contains(
            "calibration requires at least two rows, got 1",
        ));
}

#[rstest]
fn calibration_with_duplicate_raw_values_is_rejected() {
    run_with_calibration("raw,grams\n100,0.0\n100,1.0\n200,2.0\n")
        .code(exit::OTHER)
        .stderr(predicate::str::contains("duplicate raw values"));
}

#[rstest]
fn calibration_with_non_monotonic_raw_values_is_rejected() {
    run_with_calibration("raw,grams\n100,0.0\n300,2.0\n200,1.0\n")
        .code(exit::OTHER)
        .stderr(predicate::str::contains(
            "calibration raw values must be monotonic",
        ));
}

/// A comment line is *not* supported: `load_calibration_csv` builds its reader
/// without a comment character, so a leading `#` is read as the header row. This
/// pins the current behaviour so the (reasonable) follow-up of enabling comments
/// is a deliberate, test-visible change rather than an accident.
#[rstest]
fn calibration_comment_lines_are_not_supported() {
    run_with_calibration("# doser calibration\nraw,grams\n100,0.0\n200,1.0\n")
        .code(exit::OTHER)
        .stderr(predicate::str::contains(
            "Invalid headers in calibration CSV",
        ));
}

/// The happy path, so the rejection tests above are known to be rejecting the
/// specific defect rather than the whole calibration feature.
#[rstest]
fn a_well_formed_calibration_csv_is_accepted() {
    run_with_calibration("raw,grams\n1268400,0.0\n1272680,10.0\n1276960,20.0\n").success();
}

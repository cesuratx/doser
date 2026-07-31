//! A per-read scale timeout must surface to the operator as a scale problem,
//! not as a generic failure — and it must keep the non-abort exit code, since it
//! is a sensor fault rather than a safety abort.

mod common;

use assert_cmd::prelude::*;
use common::{Cfg, doser, exit};
use predicates::prelude::*;
use rstest::rstest;
use tempfile::tempdir;

#[rstest]
fn hx711_timeout_bubbles_to_cli() {
    let dir = tempdir().unwrap();
    let cfg = Cfg {
        sample_rate_hz: 10,
        sample_ms: 5,
        sensor_read_timeout_ms: 50,
        max_run_ms: 50,
        epsilon_g: 0.0,
        ..Cfg::default()
    }
    .write(&dir);

    doser(&cfg)
        .env("DOSER_TEST_SIM_TIMEOUT", "1")
        .args(["dose", "--grams", "0.5"])
        .assert()
        .code(exit::OTHER)
        .stderr(predicate::str::contains(
            "What happened: Scale read timed out",
        ));
}

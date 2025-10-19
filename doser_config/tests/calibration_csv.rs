use std::fs::File;
use std::io::Write;

use doser_config::{Calibration, CalibrationRow, load_calibration_csv};
use rstest::rstest;
use tempfile::tempdir;

#[rstest]
fn calibration_from_rows_two_points() {
    // Exact two-point fit
    let rows = vec![
        CalibrationRow {
            raw: 100,
            grams: 0.0,
        },
        CalibrationRow {
            raw: 200,
            grams: 100.0,
        },
    ];
    let c = Calibration::from_rows(rows).unwrap();
    assert!((c.scale_factor - 1.0).abs() < 1e-6);
    // zero point at raw=100 => offset round(-b/a) should be 100
    assert_eq!(c.offset, 100);
}

#[rstest]
fn calibration_from_rows_three_points_ols() {
    // Three points near a line grams = 2*raw - 200, exact here for determinism
    let rows = vec![
        CalibrationRow {
            raw: 100,
            grams: 0.0,
        },
        CalibrationRow {
            raw: 150,
            grams: 100.0,
        },
        CalibrationRow {
            raw: 200,
            grams: 200.0,
        },
    ];
    let c = Calibration::from_rows(rows).unwrap();
    assert!((c.scale_factor - 2.0).abs() < 1e-6);
    assert_eq!(c.offset, 100);
}

#[rstest]
fn calibration_rejects_duplicate_raw() {
    let rows = vec![
        CalibrationRow {
            raw: 100,
            grams: 0.0,
        },
        CalibrationRow {
            raw: 100,
            grams: 10.0,
        },
    ];
    let err = Calibration::from_rows(rows).expect_err("should fail on duplicate raw");
    assert!(format!("{err}").to_lowercase().contains("duplicate raw"));
}

#[rstest]
fn calibration_rejects_non_monotonic_zigzag() {
    // 100 -> 200 -> 150 is a zig-zag (not strictly monotonic)
    let rows = vec![
        CalibrationRow {
            raw: 100,
            grams: 0.0,
        },
        CalibrationRow {
            raw: 200,
            grams: 100.0,
        },
        CalibrationRow {
            raw: 150,
            grams: 70.0,
        },
    ];
    let err = Calibration::from_rows(rows).expect_err("should fail on non-monotonic raw");
    assert!(
        format!("{err}")
            .to_lowercase()
            .contains("monotonic (strictly increasing or strictly decreasing)")
    );
}

#[rstest]
fn csv_with_missing_header_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad_headers.csv");

    let mut f = File::create(&path).unwrap();
    writeln!(f, "raw,value").unwrap();
    writeln!(f, "100,0.0").unwrap();
    writeln!(f, "200,1.0").unwrap();

    let err = load_calibration_csv(&path).expect_err("should error on bad headers");
    assert!(format!("{err}").contains("headers 'raw,grams'"));
}

#[rstest]
fn csv_with_non_numeric_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad_numeric.csv");

    let mut f = File::create(&path).unwrap();
    writeln!(f, "raw,grams").unwrap();
    writeln!(f, "abc,xyz").unwrap();

    let err = load_calibration_csv(&path).expect_err("should error on non-numeric");
    assert!(format!("{err}").contains("invalid CSV row"));
}

#[rstest]
fn calibration_with_noise_and_outliers_recovers_params() {
    // Ground truth: grams = 0.5*raw - 50
    let true_gain = 0.5f32;
    let true_offset_raw = 100i64; // because grams=0 at raw=100 => zero_counts=100
    // Generate samples across a range with small Gaussian-like noise and a couple outliers
    let mut rows = Vec::new();
    for i in 0..50i64 {
        let raw = 50 + i * 10; // 50..=540
        let ideal = true_gain * (raw as f32) - 50.0;
        // pseudo-random noise in [-1.0, 1.0]
        let noise = ((i as f32 * 37.0).sin()) * 0.5;
        rows.push(doser_config::CalibrationRow {
            raw,
            grams: ideal + noise,
        });
    }
    // Inject a couple of strong outliers (> 2σ) by replacing in-range points to keep raw monotonic
    let idx1 = 15usize; // corresponds to raw around 200
    let idx2 = 35usize; // corresponds to raw around 400
    rows[idx1].grams = 500.0; // extreme high outlier
    rows[idx2].grams = -500.0; // extreme low outlier

    let c = doser_config::Calibration::from_rows(rows).unwrap();
    // scale_factor should be within 1% of true_gain
    let rel_err_gain = (c.scale_factor - true_gain).abs() / true_gain;
    assert!(rel_err_gain <= 0.01, "gain rel err {rel_err_gain}");
    // offset should be within 1% of 100 counts
    let rel_err_off =
        ((c.offset as f32) - (true_offset_raw as f32)).abs() / (true_offset_raw as f32);
    assert!(rel_err_off <= 0.01, "offset rel err {rel_err_off}");
}

#[rstest]
fn calibration_horizontal_line_errors() {
    // grams constant despite changing raw -> horizontal line (slope 0), should error
    let rows = vec![
        CalibrationRow {
            raw: 100,
            grams: 50.0,
        },
        CalibrationRow {
            raw: 200,
            grams: 50.0,
        },
        CalibrationRow {
            raw: 300,
            grams: 50.0,
        },
    ];
    let err = Calibration::from_rows(rows).expect_err("should fail on zero slope (horizontal)");
    let err_msg = format!("{err}").to_lowercase();
    assert!(
        err_msg.contains("zero slope") || err_msg.contains("invalid scale factor"),
        "Expected error about zero slope, got: {err}"
    );
}

#[rstest]
fn calibration_all_inliers_no_refit_matches_ols() {
    // Near-perfect line with small noise; no outliers beyond 2σ, refit should be skipped
    let true_gain = 1.25f32;
    let true_offset_raw = 80i64; // grams = a*raw - a*offset
    let mut rows = Vec::new();
    for i in 0..40i64 {
        let raw = 40 + i * 5; // strictly increasing
        let ideal = true_gain * (raw as f32) - true_gain * (true_offset_raw as f32);
        // tiny bounded noise to keep all points inliers
        let noise = ((i as f32 * 13.0).cos()) * 0.05;
        rows.push(CalibrationRow {
            raw,
            grams: ideal + noise,
        });
    }
    let c = Calibration::from_rows(rows).unwrap();
    let rel_err_gain = (c.scale_factor - true_gain).abs() / true_gain;
    assert!(rel_err_gain <= 0.01, "gain rel err {rel_err_gain}");
    let rel_err_off =
        ((c.offset as f32) - (true_offset_raw as f32)).abs() / (true_offset_raw as f32);
    assert!(rel_err_off <= 0.02, "offset rel err {rel_err_off}");
}

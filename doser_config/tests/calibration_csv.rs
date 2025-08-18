use std::fs::File;
use std::io::Write;

use doser_config::{load_calibration_csv, Calibration, CalibrationRow};
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
    assert!(format!("{}", err).to_lowercase().contains("duplicate raw"));
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
    assert!(format!("{}", err)
        .to_lowercase()
        .contains("monotonic (strictly increasing or strictly decreasing)"));
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
    assert!(format!("{}", err).contains("headers 'raw,grams'"));
}

#[rstest]
fn csv_with_non_numeric_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad_numeric.csv");

    let mut f = File::create(&path).unwrap();
    writeln!(f, "raw,grams").unwrap();
    writeln!(f, "abc,xyz").unwrap();

    let err = load_calibration_csv(&path).expect_err("should error on non-numeric");
    assert!(format!("{}", err).contains("invalid CSV row"));
}

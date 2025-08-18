use std::fs::File;
use std::io::Write;

use doser_config::{load_calibration_csv, Calibration, CalibrationRow};
use rstest::rstest;
use tempfile::tempdir;

#[rstest]
fn calibration_from_rows_happy_path() {
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
    let c = Calibration::from_rows(rows).unwrap_or_else(|e| panic!("from_rows: {e}"));
    assert!(c.scale_factor > 0.0);
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

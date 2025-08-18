use doser_config::{load_calibration_csv, Calibration, CalibrationRow};

#[test]
fn calibration_from_rows_happy_path() {
    let rows = vec![
        CalibrationRow {
            raw: 842913,
            grams: 0.0,
        },
        CalibrationRow {
            raw: 1024913,
            grams: 100.0,
        },
    ];
    let c = Calibration::from_rows(rows).expect("calibration compute");
    assert!(c.scale_factor.is_finite());
}

#[test]
fn csv_with_missing_header_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_headers.csv");
    std::fs::write(&path, "raw,value\n1,2\n").unwrap();
    let err = load_calibration_csv(&path).expect_err("should error on bad headers");
    assert!(format!("{}", err).contains("headers 'raw,grams'"));
}

#[test]
fn csv_with_non_numeric_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_value.csv");
    // grams is non-numeric
    std::fs::write(&path, "raw,grams\n123,abc\n456,100\n").unwrap();
    let err = load_calibration_csv(&path).expect_err("should error on bad value");
    assert!(format!("{}", err).contains("invalid CSV row"));
}

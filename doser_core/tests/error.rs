use doser_core::*;

#[test]
fn test_doser_error_display() {
    let err = DoserError::NegativeTarget;
    assert_eq!(format!("{}", err), "Negative target grams");
    let err = DoserError::MaxAttemptsExceeded;
    assert_eq!(format!("{}", err), "Max attempts exceeded");
    let err = DoserError::NegativeWeight;
    assert_eq!(format!("{}", err), "Negative weight reading");
}

#[test]
fn test_dosing_result_display() {
    let result = DosingResult {
        final_weight: 10.0,
        attempts: 5,
        error: Some(DoserError::NegativeWeight),
    };
    let display_str = format!("{}", result);
    assert!(display_str.contains("Final weight: 10.00g"));
    assert!(display_str.contains("Error: Negative weight reading"));
}

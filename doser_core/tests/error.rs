use doser_core::DosingResult;
use doser_core::error::DoserError;

#[test]
fn test_doser_error_display() {
    let err = DoserError::Config("Negative target grams".to_string());
    assert_eq!(
        format!("{}", err),
        "configuration error: Negative target grams"
    );
    let err = DoserError::Config("Max attempts exceeded".to_string());
    assert_eq!(
        format!("{}", err),
        "configuration error: Max attempts exceeded"
    );
    let err = DoserError::Hardware("Negative weight reading".to_string());
    assert_eq!(
        format!("{}", err),
        "hardware error: Negative weight reading"
    );
}

#[test]
fn test_dosing_result_display() {
    let result = DosingResult {
        final_weight: 10.0,
        attempts: 5,
        error: Some(DoserError::Hardware("Negative weight reading".to_string())),
    };
    let display_str = format!("{}", result);
    assert!(display_str.contains("Final weight: 10.00g"));
    assert!(display_str.contains("Error: hardware error: Negative weight reading"));
}

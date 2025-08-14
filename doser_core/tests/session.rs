use doser_core::*;

#[test]
fn test_dosing_session_builder_and_display() {
    let builder = DosingSessionBuilder::new()
        .target_grams(10.0)
        .max_attempts(5)
        .dt_pin(1)
        .sck_pin(2)
        .step_pin(3)
        .dir_pin(4);
    let session = builder.build().unwrap();
    let display_str = format!("{}", session);
    assert!(display_str.contains("DosingSession("));
}

#[test]
fn test_dosing_step_enum_iterator() {
    let mut w = 0.0;
    let mut iter = DosingStepEnum::Steps(DosingStep::new(
        5.0,
        || {
            w += 2.0;
            w
        },
        10,
    ));
    let mut results = vec![];
    while let Some((attempt, weight)) = iter.next() {
        results.push((attempt, weight));
    }
    assert_eq!(results, vec![(1, 2.0), (2, 4.0), (3, 6.0)]);
}

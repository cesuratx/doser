use doser_core::Doser;
use doser_core::error::BuildError;
use rstest::rstest;

#[rstest]
fn builder_missing_scale_yields_typed_build_error() {
    let err = Doser::builder()
        // missing with_scale()
        .with_target_grams(10.0)
        .try_build()
        .expect_err("should fail with MissingScale");

    match err.downcast_ref::<BuildError>() {
        Some(BuildError::MissingScale) => {}
        other => panic!("expected MissingScale, got: {other:?}"),
    }
}

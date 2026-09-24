use super::RuntimeError;

#[test]
fn body_access_has_stable_concise_display() {
    let error = RuntimeError::BodyAccess {
        message: "storage unavailable".to_string(),
    };
    assert_eq!(error.to_string(), "body access failed: storage unavailable");
}

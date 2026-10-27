use super::RuntimeError;

#[test]
fn body_access_has_stable_concise_display() {
    let error = RuntimeError::BodyAccess {
        message: "storage unavailable".to_string(),
    };
    assert_eq!(error.to_string(), "body access failed: storage unavailable");
}

#[test]
fn value_limit_errors_have_stable_concise_display() {
    assert_eq!(
        RuntimeError::MapTooDeep { limit: 64 }.to_string(),
        "map depth exceeded (64)"
    );
    assert_eq!(
        RuntimeError::ValueTooManyItems { limit: 32_768 }.to_string(),
        "value item limit exceeded (32768)"
    );
    assert_eq!(
        RuntimeError::ValueKeysTooLong { limit: 262_144 }.to_string(),
        "value map-key byte limit exceeded (262144)"
    );
    assert_eq!(
        RuntimeError::ValueWorkLimitExceeded { limit: 16_777_216 }.to_string(),
        "value work budget exceeded (16777216)"
    );
}

#[test]
fn body_capacity_has_distinct_stable_display() {
    assert_eq!(
        RuntimeError::BodyCapacityExceeded.to_string(),
        "request body storage capacity exhausted"
    );
}

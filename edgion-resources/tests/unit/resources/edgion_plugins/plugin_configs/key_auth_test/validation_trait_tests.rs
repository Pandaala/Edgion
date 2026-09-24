use super::*;

#[test]
fn validation_error_returns_field_error_without_validate_call() {
    let config = KeyAuthConfig {
        key_sources: vec![],
        ..Default::default()
    };
    assert_eq!(
        config.validation_error(),
        Some("key_sources cannot be empty - at least one key source must be specified".to_string())
    );
}

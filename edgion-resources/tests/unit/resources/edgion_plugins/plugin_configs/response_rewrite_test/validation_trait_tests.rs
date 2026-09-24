use super::*;
use crate::resources::edgion_plugins::PluginValidation;

#[test]
fn validation_error_returns_field_error_without_validate_call() {
    let config = ResponseRewriteConfig {
        status_code: Some(600),
        headers: None,
    };

    assert_eq!(config.validation_error().as_deref(), Some(INVALID_STATUS_CODE));
}

#[test]
fn validation_error_returns_none_for_valid_config() {
    let config = ResponseRewriteConfig {
        status_code: Some(200),
        headers: None,
    };
    assert!(config.validation_error().is_none());
}

#[test]
fn validation_error_returns_none_for_default_config() {
    let config = ResponseRewriteConfig::default();
    assert!(config.validation_error().is_none());
}

#[test]
fn validation_error_detects_empty_set_header_name() {
    let config = ResponseRewriteConfig {
        status_code: None,
        headers: Some(ResponseHeaderActions {
            set: Some(vec![ResponseHeaderEntry {
                name: "".to_string(),
                value: "v".to_string(),
            }]),
            add: None,
            remove: None,
            rename: None,
        }),
    };
    assert_eq!(config.validation_error().as_deref(), Some(INVALID_SET_NAME));
}

use super::*;
use crate::resources::edgion_plugins::PluginValidation;

/// Verify that `PluginValidation::validation_error()` works WITHOUT a prior `validate()` call.
/// The trait impl calls `check_fields()` directly, so no mutable state is required.
#[test]
fn validation_error_returns_field_error_without_validate_call() {
    // Empty rules list is invalid; the trait reports the error directly.
    let config = DynamicInternalUpstreamConfig {
        rules: Some(vec![]),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result.as_deref(),
        Some("Rules list is empty; omit 'rules' for direct mode")
    );
}

/// Default config (direct mode, no extract, no rules) must be valid.
#[test]
fn default_config_is_valid() {
    let config = DynamicInternalUpstreamConfig::default();
    assert!(PluginValidation::validation_error(&config).is_none());
}

/// Invalid regex in extract must surface the correct error prefix.
#[test]
fn invalid_regex_returns_error() {
    let config = DynamicInternalUpstreamConfig {
        extract: Some(DynUpstreamExtract {
            regex: "[invalid".to_string(),
            group: 1,
        }),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert!(result.is_some());
    assert!(
        result.as_deref().unwrap().starts_with("Invalid extract regex:"),
        "unexpected error: {:?}",
        result
    );
}

/// Rule with empty value must produce the correct indexed error.
#[test]
fn rule_with_empty_value_returns_error() {
    let config = DynamicInternalUpstreamConfig {
        rules: Some(vec![DynUpstreamRule {
            value: "".to_string(),
            backend_ref: DynUpstreamTarget {
                name: "svc".to_string(),
                namespace: None,
            },
        }]),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert_eq!(result.as_deref(), Some("Rule[0]: value cannot be empty"));
}

/// Rule with empty backend_ref name must produce the correct indexed error.
#[test]
fn rule_with_empty_backend_ref_name_returns_error() {
    let config = DynamicInternalUpstreamConfig {
        rules: Some(vec![DynUpstreamRule {
            value: "premium".to_string(),
            backend_ref: DynUpstreamTarget {
                name: "".to_string(),
                namespace: None,
            },
        }]),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert_eq!(result.as_deref(), Some("Rule[0]: backendRef.name cannot be empty"));
}

/// validate() on a valid config with regex must compile the regex.
#[test]
fn validate_compiles_regex_on_valid_config() {
    let mut config = DynamicInternalUpstreamConfig {
        extract: Some(DynUpstreamExtract {
            regex: r"group=(\w+)".to_string(),
            group: 1,
        }),
        ..Default::default()
    };
    config.validate();
    assert!(config.compiled_regex.is_some());
}

use super::*;
use crate::resources::edgion_plugins::PluginValidation;

/// Verify that `PluginValidation::validation_error()` works WITHOUT a prior `precompile()` call.
/// The trait impl calls `check_fields()` directly, so no mutable state is required.
#[test]
fn validation_error_returns_field_error_without_validate_call() {
    // Construct an invalid config (host conflict) without calling precompile().
    let config = ProxyRewriteConfig {
        host: Some("backend.svc".to_string()),
        headers: Some(HeaderActions {
            add: None,
            set: Some(vec![HeaderEntry {
                name: "Host".to_string(),
                value: "other.svc".to_string(),
            }]),
            remove: None,
        }),
        ..Default::default()
    };

    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result,
        Some(
            "Conflict: 'host' field and 'headers.set' with Host header both configured. Use only 'host' field."
                .to_string()
        )
    );
}

#[test]
fn validation_error_returns_none_for_valid_config() {
    let config = ProxyRewriteConfig {
        uri: Some("/new/path".to_string()),
        ..Default::default()
    };

    assert!(PluginValidation::validation_error(&config).is_none());
}

#[test]
fn validation_error_returns_error_for_invalid_regex_without_validate_call() {
    let config = ProxyRewriteConfig {
        regex_uri: Some(RegexUri {
            pattern: r"[invalid".to_string(),
            replacement: "/users/$1".to_string(),
        }),
        ..Default::default()
    };

    let result = PluginValidation::validation_error(&config);
    assert!(result.is_some());
    assert!(result.unwrap().contains("Invalid regex pattern"));
}

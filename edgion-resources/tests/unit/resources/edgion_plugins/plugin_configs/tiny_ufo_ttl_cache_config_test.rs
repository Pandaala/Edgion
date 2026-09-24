use super::*;
use serde_json::json;

#[test]
fn defaults_inherit_gateway_limits() {
    let config = TinyUfoTtlCacheConfig::default();
    assert_eq!(config.max_entries, 0);
    assert_eq!(config.max_logical_bytes, None);
    assert_eq!(config.parsed_max_logical_bytes(), Ok(None));
}

#[test]
fn camel_case_fields_round_trip() {
    let value = json!({
        "maxEntries": 2048,
        "maxLogicalBytes": "8MiB"
    });
    let config: TinyUfoTtlCacheConfig = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(config.max_entries, 2048);
    assert_eq!(config.parsed_max_logical_bytes(), Ok(Some(8 * 1024 * 1024)));
    assert_eq!(serde_json::to_value(config).unwrap(), value);
}

#[test]
fn malformed_and_explicit_zero_budgets_are_rejected() {
    for (value, expected) in [
        ("invalid", "maxLogicalBytes must be a valid byte size"),
        ("0", "maxLogicalBytes must be greater than 0"),
        ("0MiB", "maxLogicalBytes must be greater than 0"),
    ] {
        let config = TinyUfoTtlCacheConfig {
            max_logical_bytes: Some(value.to_string()),
            ..Default::default()
        };
        assert_eq!(config.validation_error(), Some(expected));
    }
}

#[test]
fn absolute_and_combined_limits_are_rejected_in_the_resource_layer() {
    assert_eq!(
        TinyUfoTtlCacheConfig {
            max_entries: MAX_TINY_UFO_CACHE_ENTRIES + 1,
            ..Default::default()
        }
        .validation_error(),
        Some("TinyUFO maxEntries exceeds the safety ceiling")
    );
    assert_eq!(
        TinyUfoTtlCacheConfig {
            max_entries: 1,
            max_logical_bytes: Some("256MiB".to_string()),
        }
        .validation_error(),
        Some("TinyUFO logical budget per entry exceeds the backend weight limit")
    );
}

#[test]
fn partial_override_is_rejected_to_keep_controller_and_gateway_validation_equal() {
    for config in [
        TinyUfoTtlCacheConfig {
            max_entries: 10,
            max_logical_bytes: None,
        },
        TinyUfoTtlCacheConfig {
            max_entries: 0,
            max_logical_bytes: Some("1MiB".to_string()),
        },
    ] {
        assert_eq!(
            config.validation_error(),
            Some("TinyUFO maxEntries and maxLogicalBytes must be configured together")
        );
    }
}

use super::*;
use crate::resources::edgion_plugins::PluginValidation;

#[test]
fn validation_error_both_none() {
    let config = IpRestrictionConfig::default();
    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result,
        Some("At least one of 'allowRefs'/'denyRefs' must be non-empty".to_string())
    );
}

#[test]
fn validation_error_empty_allow_list() {
    let config = IpRestrictionConfig {
        refs: serde_json::from_value(serde_json::json!({"allowRefs":[]})).unwrap(),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result,
        Some("At least one of 'allowRefs'/'denyRefs' must be non-empty".to_string())
    );
}

#[test]
fn validation_error_empty_deny_list() {
    let config = IpRestrictionConfig {
        refs: serde_json::from_value(serde_json::json!({"denyRefs":[]})).unwrap(),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result,
        Some("At least one of 'allowRefs'/'denyRefs' must be non-empty".to_string())
    );
}

#[test]
fn validation_error_tolerates_invalid_inline_cidr() {
    // Canonical IpList admission owns CIDR validation; the consumer validates references.
    let config = IpRestrictionConfig {
        refs: serde_json::from_value(serde_json::json!({"allowRefs":[{"name":"clients"}]})).unwrap(),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert!(result.is_none());
}

#[test]
fn validation_error_valid_status_200() {
    for status in [200, 599] {
        let config = IpRestrictionConfig {
            refs: serde_json::from_value(serde_json::json!({"allowRefs":[{"name":"clients"}]})).unwrap(),
            status,
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert!(result.is_none(), "{status}");
    }
}

#[test]
fn validation_error_rejects_non_final_statuses() {
    for status in [100, 101, 199, 600] {
        let config = IpRestrictionConfig {
            refs: serde_json::from_value(serde_json::json!({"allowRefs":[{"name":"clients"}]})).unwrap(),
            status,
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert_eq!(result, Some(format!("Invalid status code: {status}")));
    }
}

#[test]
fn validation_error_valid_config() {
    let config = IpRestrictionConfig {
        refs: serde_json::from_value(serde_json::json!({"allowRefs":[{"name":"clients"}]})).unwrap(),

        status: 403,
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert!(result.is_none());
}

#[test]
fn status_field_has_range_in_json_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(IpRestrictionConfig)).unwrap();
    let props = &schema["properties"]["status"];
    assert_eq!(props["minimum"].as_f64(), Some(200.0), "status minimum should be 200");
    assert_eq!(props["maximum"].as_f64(), Some(599.0), "status maximum should be 599");
}

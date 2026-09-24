use super::*;
use crate::resources::edgion_plugins::PluginValidation;

fn group(name: &str, cidrs: &[&str]) -> IpGroup {
    IpGroup {
        name: name.to_string(),
        description: None,
        cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn validation_error_both_none() {
    let config = IpRestrictionConfig::default();
    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result,
        Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
    );
}

#[test]
fn validation_error_empty_allow_list() {
    let config = IpRestrictionConfig {
        allow: Some(vec![]),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result,
        Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
    );
}

#[test]
fn validation_error_empty_deny_list() {
    let config = IpRestrictionConfig {
        deny: Some(vec![]),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert_eq!(
        result,
        Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
    );
}

#[test]
fn validation_error_tolerates_invalid_inline_cidr() {
    // Partial-good (CIDR-only leniency): a bad inline CIDR no longer rejects the
    // plugin at admission; CIDR errors are surfaced later via status/metric.
    let config = IpRestrictionConfig {
        allow: Some(vec![group("default", &["not-an-ip"])]),
        ..Default::default()
    };
    let result = PluginValidation::validation_error(&config);
    assert!(result.is_none());
}

#[test]
fn validation_error_valid_status_200() {
    for status in [200, 599] {
        let config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["192.168.1.1"])]),
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
            allow: Some(vec![group("default", &["192.168.1.1"])]),
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
        allow: Some(vec![group("a", &["192.168.1.0/24"])]),
        deny: Some(vec![group("b", &["192.168.1.100"])]),
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

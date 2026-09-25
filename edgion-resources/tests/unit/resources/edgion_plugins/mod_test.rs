//! Tests for EdgionPlugins module

use super::*;
use crate::resources::common::log_labels::LOG_LABEL_MAX_COUNT;
use crate::resources::http_route::{HTTPHeader, HTTPHeaderFilter};

/// Helper function to create EdgionPlugins for testing
fn create_edgion_plugins_with_request_filters(plugins: Option<Vec<RequestFilterEntry>>) -> EdgionPlugins {
    EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: plugins,
            ..Default::default()
        },
        status: None,
    }
}

/// Helper function to create a header modifier plugin
fn make_header_modifier_plugin() -> EdgionPlugin {
    EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
        set: Some(vec![HTTPHeader {
            name: "X-Test".into(),
            value: "test-value".into(),
        }]),
        add: None,
        remove: None,
    })
}

#[test]
fn test_has_plugins_empty() {
    let ep = create_edgion_plugins_with_request_filters(None);
    assert!(!ep.has_plugins());
    assert_eq!(ep.plugin_count(), 0);
}

#[test]
fn test_has_plugins_with_empty_vec() {
    let ep = create_edgion_plugins_with_request_filters(Some(vec![]));
    assert!(!ep.has_plugins());
    assert_eq!(ep.plugin_count(), 0);
}

#[test]
fn waf_is_one_top_level_logical_plugin_and_round_trips() {
    let yaml = r#"
waf:
  policyRef:
    name: application-waf
  mode: detectionOnly
"#;
    let spec: EdgionPluginsSpec = serde_yaml::from_str(yaml).expect("WAF configuration parses");
    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec,
        status: None,
    };

    assert!(ep.has_plugins());
    assert_eq!(ep.plugin_count(), 1);
    assert_eq!(ep.waf().map(|waf| waf.mode), Some(WafMode::DetectionOnly));
    assert!(ep.validate_plugins_before_waf_resolution().is_empty());
    assert!(
        !ep.validate_plugins().is_empty(),
        "policy references require Controller resolution"
    );

    let serialized = serde_json::to_value(&ep.spec).expect("WAF configuration serializes");
    assert_eq!(serialized["waf"]["mode"], "detectionOnly");
    assert!(serialized.get("requestPlugins").is_none());
}

#[test]
fn waf_rejects_unknown_configuration_fields() {
    let yaml = r#"
waf:
  policyRef:
    name: application-waf
  unexpected: true
"#;
    assert!(serde_yaml::from_str::<EdgionPluginsSpec>(yaml).is_err());
}

#[test]
fn test_plugin_entry_default_enabled() {
    let entry = RequestFilterEntry::new(make_header_modifier_plugin());
    assert!(entry.is_enabled());
    assert_eq!(entry.type_name(), "RequestHeaderModifier");
}

#[test]
fn test_plugin_entry_disabled() {
    let entry = RequestFilterEntry::with_enable(make_header_modifier_plugin(), false);
    assert!(!entry.is_enabled());
}

#[test]
fn test_plugin_entry_serialization() {
    // Enabled plugin (enable field should be omitted)
    let enabled_entry = RequestFilterEntry::new(make_header_modifier_plugin());
    let json = serde_json::to_string(&enabled_entry).unwrap();
    assert!(!json.contains("\"enable\"")); // enable=true is skipped
    assert!(json.contains("\"type\":\"RequestHeaderModifier\""));
    assert!(json.contains("\"config\""));

    // Disabled plugin (enable field should be present)
    let disabled_entry = RequestFilterEntry::with_enable(make_header_modifier_plugin(), false);
    let json = serde_json::to_string(&disabled_entry).unwrap();
    assert!(json.contains("\"enable\":false"));
}

#[test]
fn test_plugin_entry_deserialization() {
    // With enable=false
    let json =
        r#"{"enable":false,"type":"RequestHeaderModifier","config":{"set":[{"name":"X-Test","value":"test-value"}]}}"#;
    let entry: RequestFilterEntry = serde_json::from_str(json).unwrap();
    assert!(!entry.is_enabled());
    assert_eq!(entry.type_name(), "RequestHeaderModifier");

    // Without enable field (should default to true)
    let json = r#"{"type":"RequestHeaderModifier","config":{"set":[{"name":"X-Test","value":"test-value"}]}}"#;
    let entry: RequestFilterEntry = serde_json::from_str(json).unwrap();
    assert!(entry.is_enabled());
}

#[test]
fn test_enabled_plugins_filter() {
    let request_plugins = vec![
        RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        })),
        RequestFilterEntry::with_enable(
            EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
                set: None,
                add: Some(vec![HTTPHeader {
                    name: "X-Response".into(),
                    value: "added".into(),
                }]),
                remove: None,
            }),
            false, // disabled
        ),
    ];

    let response_filter_plugins = vec![UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        },
    ))];

    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(request_plugins),
            upstream_response_filter_plugins: Some(response_filter_plugins),
            ..Default::default()
        },
        status: None,
    };

    assert_eq!(ep.plugin_count(), 3);

    // Test individual stage counts
    let request_entries = ep.spec.request_plugins.as_ref().unwrap();
    let enabled_request = request_entries.iter().filter(|e| e.is_enabled()).count();
    assert_eq!(enabled_request, 1);
}

#[test]
fn test_edgion_plugin_type_name() {
    let plugin = make_header_modifier_plugin();
    assert_eq!(plugin.type_name(), "RequestHeaderModifier");

    if let EdgionPlugin::RequestHeaderModifier(config) = plugin {
        assert!(config.set.is_some());
        assert_eq!(config.set.unwrap()[0].name, "X-Test");
    } else {
        panic!("Expected RequestHeaderModifier variant");
    }
}

#[test]
fn access_log_extern_valid_declaration_passes_validation() {
    let yaml = r#"
accessLogExtern:
  - key: tenant
    from: ctx
    name: tenant_id
  - key: ua
    from: header
    name: user-agent
"#;
    let spec: EdgionPluginsSpec = serde_yaml::from_str(yaml).unwrap();
    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec,
        status: None,
    };
    assert!(ep.validate_plugins().is_empty(), "{:?}", ep.validate_plugins());
    assert_eq!(ep.access_log_extern_fields().len(), 2);
}

#[test]
fn access_log_extern_blocked_header_is_reported_at_ingest() {
    let yaml = r#"
accessLogExtern:
  - key: auth
    from: header
    name: Authorization
"#;
    let spec: EdgionPluginsSpec = serde_yaml::from_str(yaml).unwrap();
    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec,
        status: None,
    };
    let errors = ep.validate_plugins();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("accessLogExtern[0]"), "{errors:?}");
    assert!(errors[0].contains("blocked"), "{errors:?}");
}

#[test]
fn access_log_extern_duplicate_key_is_reported_at_ingest() {
    // Silent last-wins on YAML order would be undebuggable; report instead.
    let yaml = r#"
accessLogExtern:
  - key: tenant
    from: ctx
    name: tenant_id
  - key: tenant
    from: header
    name: x-tenant
"#;
    let spec: EdgionPluginsSpec = serde_yaml::from_str(yaml).unwrap();
    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec,
        status: None,
    };
    let errors = ep.validate_plugins();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("duplicate key"), "{errors:?}");
}

#[test]
fn access_log_extern_over_count_cap_is_reported_at_ingest() {
    let mut yaml = String::from("accessLogExtern:\n");
    for i in 0..(LOG_LABEL_MAX_COUNT + 1) {
        yaml.push_str(&format!("  - key: k{i}\n    from: ctx\n    name: n{i}\n"));
    }
    let spec: EdgionPluginsSpec = serde_yaml::from_str(&yaml).unwrap();
    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec,
        status: None,
    };
    let errors = ep.validate_plugins();
    assert!(
        errors.iter().any(|e| e.contains("at most")),
        "expected a count-cap error, got {errors:?}"
    );
}

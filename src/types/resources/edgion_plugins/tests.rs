//! Tests for EdgionPlugins module

use super::*;
use crate::types::resources::http_route::{HTTPHeader, HTTPHeaderFilter};

/// Helper function to create EdgionPlugins for testing
fn create_edgion_plugins_with_request_filters(plugins: Option<Vec<RequestFilterEntry>>) -> EdgionPlugins {
    let mut ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            request_plugins: plugins,
            ..Default::default()
        },
        status: None,
    };
    ep.preparse();
    ep
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

    let mut ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            request_plugins: Some(request_plugins),
            upstream_response_filter_plugins: Some(response_filter_plugins),
            ..Default::default()
        },
        status: None,
    };
    ep.preparse();

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

/// Regression test for stage-mismatch detection (fixes #14).
///
/// A `ResponseHeaderModifier` plugin placed under `requestPlugins` is a response-stage
/// plugin mis-configured into the request stage. Before the fix, PluginRuntime silently
/// dropped such entries, leaving `preparse_errors` empty and `status.conditions` reporting
/// `Accepted: True` even though the plugin never ran (auth-bypass class bug).
///
/// After the fix, preparse must surface the mismatch in `preparse_errors` so the
/// Controller writes it into `status.conditions`.
#[test]
fn test_preparse_flags_stage_mismatch() {
    // ResponseHeaderModifier → request stage (wrong stage)
    let request_plugins = vec![RequestFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        },
    ))];

    // RequestHeaderModifier → upstream response filter stage (wrong stage)
    let response_filter_plugins = vec![UpstreamResponseFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        },
    ))];

    let mut ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            request_plugins: Some(request_plugins),
            upstream_response_filter_plugins: Some(response_filter_plugins),
            ..Default::default()
        },
        status: None,
    };
    ep.preparse();

    let errors = ep.get_preparse_errors();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("requestPlugins[0]") && e.contains("not valid for request stage")),
        "expected request-stage mismatch error, got: {:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.contains("upstreamResponseFilterPlugins[0]")
            && e.contains("not valid for upstream response filter stage")),
        "expected upstream-response-filter-stage mismatch error, got: {:?}",
        errors
    );
}

/// Regression test: valid entries must NOT produce stage-mismatch errors.
#[test]
fn test_preparse_no_false_positive_on_valid_stages() {
    let request_plugins = vec![RequestFilterEntry::new(make_header_modifier_plugin())];
    let response_filter_plugins = vec![UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        },
    ))];

    let mut ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            request_plugins: Some(request_plugins),
            upstream_response_filter_plugins: Some(response_filter_plugins),
            ..Default::default()
        },
        status: None,
    };
    ep.preparse();

    let errors = ep.get_preparse_errors();
    assert!(
        !errors.iter().any(|e| e.contains("not valid for")),
        "valid stage placement should not produce mismatch errors, got: {:?}",
        errors
    );
}

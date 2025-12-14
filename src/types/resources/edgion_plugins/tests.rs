//! Tests for EdgionPlugins module

use super::*;
use crate::types::resources::http_route::{HTTPHeader, HTTPHeaderFilter};

/// Helper function to create EdgionPlugins for testing
fn create_edgion_plugins(plugins: Option<Vec<PluginEntry>>) -> EdgionPlugins {
    let mut ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            plugins,
            plugin_runtime: Default::default(),
        },
        status: None,
    };
    ep.init_plugin_runtime();
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
    let ep = create_edgion_plugins(None);
    assert!(!ep.has_plugins());
    assert_eq!(ep.plugin_count(), 0);
}

#[test]
fn test_has_plugins_with_empty_vec() {
    let ep = create_edgion_plugins(Some(vec![]));
    assert!(!ep.has_plugins());
    assert_eq!(ep.plugin_count(), 0);
}

#[test]
fn test_plugin_entry_default_enabled() {
    let entry = PluginEntry::new(make_header_modifier_plugin());
    assert!(entry.is_enabled());
    assert_eq!(entry.type_name(), "RequestHeaderModifier");
}

#[test]
fn test_plugin_entry_disabled() {
    let entry = PluginEntry::with_enable(make_header_modifier_plugin(), false);
    assert!(!entry.is_enabled());
}

#[test]
fn test_plugin_entry_serialization() {
    // Enabled plugin (enable field should be omitted)
    let enabled_entry = PluginEntry::new(make_header_modifier_plugin());
    let json = serde_json::to_string(&enabled_entry).unwrap();
    assert!(!json.contains("\"enable\"")); // enable=true is skipped
    assert!(json.contains("\"type\":\"requestHeaderModifier\""));
    assert!(json.contains("\"config\""));

    // Disabled plugin (enable field should be present)
    let disabled_entry = PluginEntry::with_enable(make_header_modifier_plugin(), false);
    let json = serde_json::to_string(&disabled_entry).unwrap();
    assert!(json.contains("\"enable\":false"));
}

#[test]
fn test_plugin_entry_deserialization() {
    // With enable=false
    let json = r#"{"enable":false,"type":"requestHeaderModifier","config":{"set":[{"name":"X-Test","value":"test-value"}]}}"#;
    let entry: PluginEntry = serde_json::from_str(json).unwrap();
    assert!(!entry.is_enabled());
    assert_eq!(entry.type_name(), "RequestHeaderModifier");

    // Without enable field (should default to true)
    let json = r#"{"type":"requestHeaderModifier","config":{"set":[{"name":"X-Test","value":"test-value"}]}}"#;
    let entry: PluginEntry = serde_json::from_str(json).unwrap();
    assert!(entry.is_enabled());
}

#[test]
fn test_enabled_plugins_filter() {
    let plugins = vec![
        PluginEntry::new(EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
            set: None,
            add: None,
            remove: Some(vec!["X-Remove".into()]),
        })),
        PluginEntry::with_enable(
            EdgionPlugin::ResponseHeaderModifier(HTTPHeaderFilter {
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

    let ep = create_edgion_plugins(Some(plugins));
    assert_eq!(ep.plugin_count(), 2);

    let enabled = ep.enabled_plugins();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].type_name(), "RequestHeaderModifier");
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


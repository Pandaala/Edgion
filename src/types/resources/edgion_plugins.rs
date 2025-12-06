//! EdgionPlugins custom resource definition
//!
//! EdgionPlugins defines reusable plugin configurations that can be referenced by HTTPRoutes

use std::sync::Arc;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::filters::PluginRuntime;
use super::http_route::{
    HTTPHeaderFilter, HTTPRequestMirrorFilter, HTTPRequestRedirectFilter,
    HTTPURLRewriteFilter, LocalObjectReference,
};

/// API group for EdgionPlugins
pub const EDGION_PLUGINS_GROUP: &str = "edgion.io";

/// Kind for EdgionPlugins
pub const EDGION_PLUGINS_KIND: &str = "EdgionPlugins";

/// EdgionPlugins defines reusable plugin configurations
#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionPlugins",
    plural = "edgionplugins",
    shortname = "eplugins",
    namespaced,
    status = "EdgionPluginsStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionPluginsSpec {
    // /// ParentRefs references the resources that this plugin wants to be attached to
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub parent_refs: Option<Vec<ParentReference>>,

    /// Plugin configurations
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginEntry>>,

    /// Plugin runtime (runtime only, not serialized)
    /// This is computed from plugins at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,
}

/// Status of EdgionPlugins
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct EdgionPluginsStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl EdgionPlugins {
    /// Get the namespace of this resource
    pub fn namespace(&self) -> Option<&str> {
        self.metadata.namespace.as_deref()
    }

    /// Get the name of this resource
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("")
    }

    /// Check if this plugin has any filters defined
    pub fn has_plugins(&self) -> bool {
        self.spec.plugins.as_ref().map_or(false, |p| !p.is_empty())
    }

    /// Get the total number of filters
    pub fn plugin_count(&self) -> usize {
        self.spec.plugins.as_ref().map_or(0, |p| p.len())
    }

    /// Get plugin entries as a slice
    pub fn plugin_entries(&self) -> &[PluginEntry] {
        self.spec.plugins.as_deref().unwrap_or(&[])
    }

    /// Get only enabled filters
    pub fn enabled_plugins(&self) -> Vec<&EdgionPlugin> {
        self.spec
            .plugins
            .as_ref()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| e.enable)
                    .map(|e| &e.plugin)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Initialize plugin runtime from plugins
    /// This should be called after deserialization to populate the runtime field
    pub fn init_plugin_runtime(&mut self) {
        if let Some(plugins) = &self.spec.plugins {
            self.spec.plugin_runtime = Arc::new(PluginRuntime::from_edgion_plugins(plugins));
        }
    }
}

// ============================================================================
// PluginEntry - Wrapper with enable switch
// ============================================================================

/// Helper functions for serde defaults
fn default_true() -> bool { true }
fn is_true(v: &bool) -> bool { *v }

/// Conditional enable configuration
/// 
/// YAML format:
/// ```yaml
/// conditionEnable:
///   timeBefore: "2024-12-31T23:59:59Z"   # Plugin enabled before this time
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConditionEnable {
    /// Plugin is enabled before this time (RFC3339 format)
    /// Example: "2024-12-31T23:59:59Z"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_before: Option<String>,
}

impl ConditionEnable {
    /// Check if the condition is satisfied at the current time
    pub fn is_satisfied(&self) -> bool {
        if let Some(time_before) = &self.time_before {
            if let Ok(deadline) = chrono::DateTime::parse_from_rfc3339(time_before) {
                return chrono::Utc::now() < deadline;
            }
        }
        // If no condition or parse error, default to true
        true
    }
}

/// Plugin entry with enable switch
/// 
/// YAML format:
/// ```yaml
/// filters:
///   - enable: true                    # optional, defaults to true
///     type: requestHeaderModifier
///     config:
///       set:
///         - name: X-Test
///           value: test-value
///   - enable: false                   # disabled plugin
///     type: responseHeaderModifier
///     config:
///       add:
///         - name: X-Response
///           value: added
///   - conditionEnable:                # conditional enable
///       timeBefore: "2024-12-31T23:59:59Z"
///     type: requestRedirect
///     config:
///       hostname: example.com
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Conditional enable configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl PluginEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            plugin,
        }
    }

    /// Create a new plugin entry with condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering both enable flag and condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

// ============================================================================
// EdgionPlugin - Enum representation for all plugin types
// ============================================================================

/// Plugin enum for all supported plugin types
/// 
/// Naming convention:
/// - Gateway API standard filters: keep original names (RequestHeaderModifier, etc.)
/// - Custom Edgion filters: use EdgionXxx naming (EdgionRateLimit, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config", rename_all = "camelCase")]
pub enum EdgionPlugin {
    // ========== Gateway API standard filters ==========
    RequestHeaderModifier(HTTPHeaderFilter),
    ResponseHeaderModifier(HTTPHeaderFilter),
    RequestRedirect(HTTPRequestRedirectFilter),
    UrlRewrite(HTTPURLRewriteFilter),
    RequestMirror(HTTPRequestMirrorFilter),
    ExtensionRef(LocalObjectReference),

    // ========== Custom Edgion filters ==========
    // TODO: Add custom Edgion filters here
    // EdgionRateLimit(RateLimitConfig),
    // EdgionCircuitBreaker(CircuitBreakerConfig),
    // EdgionAuth(AuthConfig),
    // EdgionWaf(WafConfig),
    // ...
}

impl EdgionPlugin {
    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        match self {
            EdgionPlugin::RequestHeaderModifier(_) => "RequestHeaderModifier",
            EdgionPlugin::ResponseHeaderModifier(_) => "ResponseHeaderModifier",
            EdgionPlugin::RequestRedirect(_) => "RequestRedirect",
            EdgionPlugin::UrlRewrite(_) => "UrlRewrite",
            EdgionPlugin::RequestMirror(_) => "RequestMirror",
            EdgionPlugin::ExtensionRef(_) => "ExtensionRef",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::http_route::HTTPHeader;

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
}

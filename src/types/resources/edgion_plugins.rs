//! EdgionPlugins custom resource definition
//!
//! EdgionPlugins defines reusable plugin configurations that can be referenced by HTTPRoutes

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::http_route::{
    HTTPHeaderFilter, HTTPRequestMirrorFilter, HTTPRequestRedirectFilter,
    HTTPURLRewriteFilter, LocalObjectReference, ParentReference,
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
    /// ParentRefs references the resources that this plugin wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Plugin configurations
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<EdgionPlugin>>,
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

    /// Check if this plugin has any plugins defined
    pub fn has_plugins(&self) -> bool {
        self.spec.plugins.as_ref().map_or(false, |p| !p.is_empty())
    }

    /// Get the total number of plugins
    pub fn plugin_count(&self) -> usize {
        self.spec.plugins.as_ref().map_or(0, |p| p.len())
    }

    /// Get plugins as a slice
    pub fn plugins(&self) -> &[EdgionPlugin] {
        self.spec.plugins.as_deref().unwrap_or(&[])
    }
}

// ============================================================================
// EdgionPlugin - Enum representation for all plugin types
// ============================================================================

/// Plugin enum for all supported plugin types
/// 
/// YAML format:
/// ```yaml
/// plugins:
///   - type: RequestHeaderModifier
///     config:
///       set:
///         - name: X-Test
///           value: test-value
///   - type: EdgionRateLimit
///     config:
///       requestsPerSecond: 100
/// ```
/// 
/// Naming convention:
/// - Gateway API standard filters: keep original names (RequestHeaderModifier, etc.)
/// - Custom Edgion plugins: use EdgionXxx naming (EdgionRateLimit, etc.)
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

    // ========== Custom Edgion plugins ==========
    // TODO: Add custom Edgion plugins here
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

    fn create_edgion_plugins(plugins: Option<Vec<EdgionPlugin>>) -> EdgionPlugins {
        EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                parent_refs: None,
                plugins,
            },
            status: None,
        }
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
    fn test_edgion_plugin_type_name() {
        let plugin = EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
            set: Some(vec![HTTPHeader {
                name: "X-Test".into(),
                value: "test-value".into(),
            }]),
            add: None,
            remove: None,
        });

        assert_eq!(plugin.type_name(), "RequestHeaderModifier");

        if let EdgionPlugin::RequestHeaderModifier(config) = plugin {
            assert!(config.set.is_some());
            assert_eq!(config.set.unwrap()[0].name, "X-Test");
        } else {
            panic!("Expected RequestHeaderModifier variant");
        }
    }

    #[test]
    fn test_edgion_plugin_serialization() {
        let plugin = EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
            set: Some(vec![HTTPHeader {
                name: "X-Test".into(),
                value: "test-value".into(),
            }]),
            add: None,
            remove: None,
        });

        let json = serde_json::to_string(&plugin).unwrap();
        assert!(json.contains("\"type\":\"requestHeaderModifier\""));
        assert!(json.contains("\"config\""));

        // Deserialize back
        let deserialized: EdgionPlugin = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.type_name(), "RequestHeaderModifier");
    }

    #[test]
    fn test_edgion_plugins_with_multiple() {
        let plugins = vec![
            EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
                set: None,
                add: None,
                remove: Some(vec!["X-Remove".into()]),
            }),
            EdgionPlugin::ResponseHeaderModifier(HTTPHeaderFilter {
                set: None,
                add: Some(vec![HTTPHeader {
                    name: "X-Response".into(),
                    value: "added".into(),
                }]),
                remove: None,
            }),
        ];

        let ep = create_edgion_plugins(Some(plugins));
        assert!(ep.has_plugins());
        assert_eq!(ep.plugin_count(), 2);
        assert_eq!(ep.plugins()[0].type_name(), "RequestHeaderModifier");
        assert_eq!(ep.plugins()[1].type_name(), "ResponseHeaderModifier");
    }
}

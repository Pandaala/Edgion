//! EdgionStreamPlugins custom resource definition
//!
//! EdgionStreamPlugins defines reusable plugin configurations for stream routes (TCP/UDP/TLS).
//!
//! Two-stage plugin model:
//! - `plugins` (Stage 1): ConnectionFilter — pre-TLS, IP-only
//! - `tls_route_plugins` (Stage 2): TlsRoute — post-handshake, post-route-match

use crate::resources::common::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Submodules
pub mod connection_ip_restriction_rules;
pub mod connection_rate_limit_config;
pub mod geo_rules;
pub mod global_connection_ip_restriction;
pub mod stream_plugins;
pub mod tls_route_plugins;

// Re-exports from edgion_plugins
pub use crate::resources::edgion_plugins::PluginEntry;
pub use connection_ip_restriction_rules::ConnectionIpRestrictionRules;
pub use connection_rate_limit_config::{
    ConnectionRateLimitConfig, PerListenerLimit, PerSourceIpLimit, MAX_REDIS_ACCEPT_TIMEOUT_MS,
};
pub use geo_rules::GeoRules;
pub use global_connection_ip_restriction::{GlobalConnectionIpRestrictionConfig, ProfileRules};
pub use stream_plugins::EdgionStreamPlugin;
pub use tls_route_plugins::TlsRouteStreamPlugin;

// Re-export plugin configs
pub use crate::resources::edgion_plugins::DefaultAction;

/// EdgionStreamPlugins defines reusable plugin configurations for stream protocols
#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionStreamPlugins",
    plural = "edgionstreamplugins",
    shortname = "esplugins",
    namespaced,
    status = "EdgionStreamPluginsStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionStreamPluginsSpec {
    /// Stage 1 (ConnectionFilter) plugin configurations — pre-TLS, IP-only
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<StreamPluginEntry>>,

    /// Stage 2 (TlsRoute) plugin configurations — post-handshake, post-route-match
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_route_plugins: Option<Vec<TlsRoutePluginEntry>>,
}

/// Stream plugin entry with enable switch
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamPluginEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Set by the controller when the plugin policy matches this entry.
    /// Never operator-facing: skipped in the CRD schema, carried to the
    /// data plane via conf_sync. The controller clears and re-stamps it on
    /// every preparse, so a hand-written value can never survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub policy_action: Option<crate::resources::plugin_policy::PluginPolicyAction>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionStreamPlugin,
}

/// Helper functions for serde defaults
fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

impl StreamPluginEntry {
    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// TLS route stage plugin entry with enable switch
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TlsRoutePluginEntry {
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Set by the controller when the plugin policy matches this entry.
    /// Never operator-facing: skipped in the CRD schema, carried to the
    /// data plane via conf_sync. The controller clears and re-stamps it on
    /// every preparse, so a hand-written value can never survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub policy_action: Option<crate::resources::plugin_policy::PluginPolicyAction>,

    #[serde(flatten)]
    pub plugin: TlsRouteStreamPlugin,
}

impl TlsRoutePluginEntry {
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// Status of EdgionStreamPlugins
#[derive(Default, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
pub struct EdgionStreamPluginsStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl EdgionStreamPlugins {
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
        self.spec.plugins.as_ref().is_some_and(|p| !p.is_empty())
    }

    /// Get the total number of plugins
    pub fn plugin_count(&self) -> usize {
        self.spec.plugins.as_ref().map_or(0, |p| p.len())
    }
}

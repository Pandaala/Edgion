//! EdgionStreamPlugins custom resource definition
//!
//! EdgionStreamPlugins defines reusable plugin configurations for stream routes (TCP/UDP)

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::gateway::plugins::StreamPluginRuntime;

// Submodules
pub mod stream_plugins;

// Re-exports from edgion_plugins
pub use crate::types::resources::edgion_plugins::PluginEntry;
pub use stream_plugins::EdgionStreamPlugin;

// Re-export plugin configs
pub use crate::types::resources::edgion_plugins::{DefaultAction, IpRestrictionConfig, IpSource};

/// API group for EdgionStreamPlugins
pub const EDGION_STREAM_PLUGINS_GROUP: &str = "edgion.io";

/// Kind for EdgionStreamPlugins
pub const EDGION_STREAM_PLUGINS_KIND: &str = "EdgionStreamPlugins";

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
    /// Plugin configurations
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<StreamPluginEntry>>,

    /// Plugin runtime (runtime only, not serialized)
    /// This is computed from plugins at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub stream_plugin_runtime: Arc<StreamPluginRuntime>,
}

/// Stream plugin entry with enable switch
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamPluginEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

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

    /// Get plugin entries as a slice
    pub fn plugin_entries(&self) -> &[StreamPluginEntry] {
        self.spec.plugins.as_deref().unwrap_or(&[])
    }

    /// Get only enabled plugins
    pub fn enabled_plugins(&self) -> Vec<&EdgionStreamPlugin> {
        self.spec
            .plugins
            .as_ref()
            .map(|entries| entries.iter().filter(|e| e.is_enabled()).map(|e| &e.plugin).collect())
            .unwrap_or_default()
    }

    /// Initialize plugin runtime from plugins
    /// This should be called after deserialization to populate the runtime field
    pub fn init_stream_plugin_runtime(&mut self) {
        if let Some(plugins) = &self.spec.plugins {
            self.spec.stream_plugin_runtime = Arc::new(StreamPluginRuntime::from_stream_plugins(plugins));
        }
    }
}

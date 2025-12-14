//! EdgionPlugins custom resource definition
//!
//! EdgionPlugins defines reusable plugin configurations that can be referenced by HTTPRoutes

use std::sync::Arc;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::filters::PluginRuntime;

// Submodules
pub mod entry;
pub mod gateway_api_plugins;
pub mod custom_plugins;

#[cfg(test)]
mod tests;

// Re-exports
pub use entry::{ConditionEnable, PluginEntry};
pub use gateway_api_plugins::EdgionPlugin;

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


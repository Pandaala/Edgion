//! Stream plugin runtime for executing stream plugins

use super::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::plugins::stream::{ConnectionRateLimitPlugin, GlobalConnectionIpRestrictionPlugin, StreamIpRestriction};
use edgion_resources::resources::edgion_stream_plugins::{EdgionStreamPlugin, StreamPluginEntry};
use std::sync::Arc;

/// Runtime for executing stream plugins
#[derive(Clone)]
pub struct StreamPluginRuntime {
    /// Ordered list of plugins to execute
    plugins: Vec<Arc<dyn StreamPlugin>>,
}

impl std::fmt::Debug for StreamPluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamPluginRuntime")
            .field("plugin_count", &self.plugins.len())
            .finish()
    }
}

impl Default for StreamPluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamPluginRuntime {
    /// Create an empty runtime
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    /// Create runtime from stream plugin entries.
    ///
    /// `parent_namespace` is the namespace of the owning `EdgionStreamPlugins` resource,
    /// used by `GlobalConnectionIpRestrictionPlugin` to resolve refs with no explicit namespace.
    pub fn from_stream_plugins(entries: &[StreamPluginEntry], parent_namespace: &str) -> Self {
        let mut plugins: Vec<Arc<dyn StreamPlugin>> = Vec::new();

        for entry in entries {
            if !entry.is_enabled() {
                continue;
            }

            let plugin: Arc<dyn StreamPlugin> = match &entry.plugin {
                EdgionStreamPlugin::IpRestriction(rules) => Arc::new(StreamIpRestriction::new(rules)),
                EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                    Arc::new(GlobalConnectionIpRestrictionPlugin::new(
                        gcir.plugin_metadata_ref.clone(),
                        parent_namespace.to_string(),
                    ))
                }
                EdgionStreamPlugin::ConnectionRateLimit(cfg) => Arc::new(ConnectionRateLimitPlugin::new(cfg)),
            };

            plugins.push(plugin);
        }

        Self { plugins }
    }

    /// Execute all plugins in order
    /// Returns Deny on first plugin that denies, otherwise Allow
    pub async fn run(&self, ctx: &StreamContext) -> StreamPluginResult {
        for plugin in &self.plugins {
            match plugin.on_connection(ctx).await {
                StreamPluginResult::Allow => {
                    // Continue to next plugin
                    continue;
                }
                StreamPluginResult::Deny(reason) => {
                    return StreamPluginResult::Deny(reason);
                }
            }
        }

        // All plugins passed
        StreamPluginResult::Allow
    }

    /// Get the number of plugins in this runtime
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Check if runtime has any plugins
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

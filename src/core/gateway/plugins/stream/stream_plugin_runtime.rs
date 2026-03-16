//! Stream plugin runtime for executing stream plugins

use super::ip_restriction::StreamIpRestriction;
use super::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::types::resources::edgion_stream_plugins::{EdgionStreamPlugin, StreamPluginEntry};
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

    /// Create runtime from stream plugin entries
    pub fn from_stream_plugins(entries: &[StreamPluginEntry]) -> Self {
        let mut plugins: Vec<Arc<dyn StreamPlugin>> = Vec::new();

        for entry in entries {
            // Skip disabled plugins
            if !entry.is_enabled() {
                tracing::debug!(plugin_type = entry.type_name(), "Skipping disabled stream plugin");
                continue;
            }

            // Create plugin based on type
            let plugin: Option<Arc<dyn StreamPlugin>> = match &entry.plugin {
                EdgionStreamPlugin::IpRestriction(config) => Some(Arc::new(StreamIpRestriction::new(config))),
            };

            if let Some(p) = plugin {
                tracing::debug!(plugin_name = p.name(), "Added stream plugin to runtime");
                plugins.push(p);
            }
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

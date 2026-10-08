//! Stream plugin runtime for executing stream plugins

use super::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::plugins::stream::{
    ConnectionRateLimitPlugin, GeoIpLocationStreamPlugin, GlobalConnectionIpRestrictionPlugin, StreamIpRestriction,
};
use edgion_resources::refs::ip_list_fields::StreamIpListBindings;
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::resources::edgion_stream_plugins::{EdgionStreamPlugin, StreamPluginEntry};
use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily};
use std::sync::Arc;

pub(super) type StreamEntryOutcome = Result<Arc<dyn StreamPlugin>, &'static str>;

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

    /// Reject every attachment of a source containing an unsupported type.
    pub(crate) fn unsupported_source() -> Self {
        Self {
            plugins: vec![Arc::new(FailClosedStreamPlugin {
                reason: "unsupported stream plugin type".to_string(),
            })],
        }
    }

    /// The caller binds these slots to one immutable source generation. Only
    /// allowed entries acquire executable references in the resulting view.
    pub(super) fn from_owner_cached(
        binding: &StreamIpListBindings<'_>,
        resource_key: &str,
        policy: Option<&CompiledPluginPolicy>,
        cache: &mut [Option<StreamEntryOutcome>],
    ) -> Self {
        let entries = binding.owner().spec.plugins.as_deref().unwrap_or_default();
        let mut plugins: Vec<Arc<dyn StreamPlugin>> = Vec::new();
        // First policy Block reason encountered, if any. Inserted at the
        // front of `plugins` after the loop so it dominates the chain
        // regardless of the restricted entry's original position (mirrors the
        // HTTP "gate first" semantics).
        let mut policy_block_reason: Option<String> = None;
        let mut invalid_config_reason: Option<String> = None;

        for (index, entry) in entries.iter().enumerate() {
            if matches!(entry.plugin, EdgionStreamPlugin::Unsupported) {
                invalid_config_reason.get_or_insert_with(|| "unsupported stream plugin type".to_string());
                continue;
            }
            if !entry.is_enabled() {
                continue;
            }

            // Policy is selected by the admitted Gateway scope before entry
            // construction. Stream admission has no HTTP response status.
            let action = policy.and_then(|policy| policy.action_for(PluginFamily::Stream, entry.type_name()));
            match action {
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { .. }) => {
                    if policy_block_reason.is_none() {
                        policy_block_reason = Some(format!("plugin disabled by policy: stream/{}", entry.type_name()));
                    }
                    continue;
                }
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass) => {
                    continue;
                }
                None => {}
            }

            let outcome = cache[index].get_or_insert_with(|| {
                let outcome = Self::build_entry(entry, binding, resource_key, index);
                if outcome.is_err() {
                    tracing::error!(
                        component = "stream_plugin_runtime",
                        plugin = entry.type_name(),
                        resource_key,
                        error_class = "invalid configuration",
                        "Invalid stream plugin configuration; all connections will be denied",
                    );
                }
                outcome
            });
            match outcome {
                Ok(plugin) => plugins.push(Arc::clone(plugin)),
                Err(reason) => {
                    invalid_config_reason.get_or_insert_with(|| (*reason).to_owned());
                }
            }
        }

        if let Some(reason) = policy_block_reason.or(invalid_config_reason) {
            plugins.insert(0, Arc::new(FailClosedStreamPlugin { reason }));
        }

        Self { plugins }
    }

    fn build_entry(
        entry: &StreamPluginEntry,
        binding: &StreamIpListBindings<'_>,
        resource_key: &str,
        index: usize,
    ) -> StreamEntryOutcome {
        match &entry.plugin {
            EdgionStreamPlugin::Unsupported => Err("unsupported stream plugin type"),
            EdgionStreamPlugin::IpRestriction(config) => {
                if !config.legacy_http_fields().is_empty() || config.validation_error().is_some() {
                    return Err("invalid stream/IpRestriction configuration");
                }
                StreamIpRestriction::from_bound(binding.connection_entry(index).ok_or("invalid owning IP reference")?)
                    .map(|plugin| Arc::new(plugin) as Arc<dyn StreamPlugin>)
                    .map_err(|_| "invalid owning IP reference")
            }
            EdgionStreamPlugin::GlobalConnectionIpRestriction(_) => GlobalConnectionIpRestrictionPlugin::from_bound(
                binding.global_entry(index).ok_or("invalid owning IP reference")?,
            )
            .map(|plugin| Arc::new(plugin) as Arc<dyn StreamPlugin>)
            .map_err(|_| "invalid stream/GlobalConnectionIpRestriction configuration"),
            EdgionStreamPlugin::ConnectionRateLimit(config) => {
                if config.validation_error().is_some() {
                    return Err("invalid stream/ConnectionRateLimit configuration");
                }
                Ok(Arc::new(ConnectionRateLimitPlugin::new(
                    config,
                    &crate::plugins::common::entry_scope(resource_key, None, index),
                )))
            }
            EdgionStreamPlugin::GeoIpLocation(config) => GeoIpLocationStreamPlugin::new(config)
                .map(|plugin| Arc::new(plugin) as Arc<dyn StreamPlugin>)
                .map_err(|_| "invalid stream/GeoIpLocation configuration"),
        }
    }

    /// Execute all plugins in order
    /// Returns Deny on first plugin that denies, otherwise Allow
    pub async fn run(&self, ctx: &StreamContext) -> StreamPluginResult {
        for plugin in &self.plugins {
            match plugin.on_connection(ctx).await {
                StreamPluginResult::Allow => continue,
                StreamPluginResult::Deny(reason) => {
                    return StreamPluginResult::Deny(reason);
                }
            }
        }

        StreamPluginResult::Allow
    }

    /// Check if runtime has any plugins
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

/// Always-deny gate for policy blocks and invalid published configurations.
struct FailClosedStreamPlugin {
    reason: String,
}

#[async_trait::async_trait]
impl StreamPlugin for FailClosedStreamPlugin {
    fn name(&self) -> &str {
        "FailClosed"
    }

    async fn on_connection(&self, _ctx: &StreamContext) -> StreamPluginResult {
        StreamPluginResult::Deny(self.reason.clone())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/stream/stream_plugin_runtime_test.rs"]
mod tests;

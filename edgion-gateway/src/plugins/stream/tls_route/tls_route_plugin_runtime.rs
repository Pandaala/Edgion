//! Runtime for executing TLS route plugins in order (first-deny-wins).

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::stream::stream_plugin_trait::StreamPluginResult;
use edgion_resources::resources::edgion_stream_plugins::{TlsRoutePluginEntry, TlsRouteStreamPlugin};
use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily};
use std::sync::Arc;

use super::ip_restriction::TlsRouteIpRestriction;

/// Runtime for executing TLS route stage plugins.
pub struct TlsRoutePluginRuntime {
    plugins: Vec<Arc<dyn TlsRoutePlugin>>,
}

impl std::fmt::Debug for TlsRoutePluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsRoutePluginRuntime")
            .field("plugin_count", &self.plugins.len())
            .finish()
    }
}

impl Default for TlsRoutePluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsRoutePluginRuntime {
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    #[cfg(test)]
    pub(crate) fn from_test_plugins(plugins: Vec<Arc<dyn TlsRoutePlugin>>) -> Self {
        Self { plugins }
    }

    /// Build runtime from TLS route plugin entries.
    pub fn from_entries(entries: &[TlsRoutePluginEntry], parent_namespace: &str) -> Self {
        Self::from_cached_entries(entries, parent_namespace, None, &mut vec![None; entries.len()])
    }

    pub(crate) fn from_cached_entries(
        entries: &[TlsRoutePluginEntry],
        parent_namespace: &str,
        policy: Option<&CompiledPluginPolicy>,
        cache: &mut [Option<Arc<dyn TlsRoutePlugin>>],
    ) -> Self {
        let mut plugins: Vec<Arc<dyn TlsRoutePlugin>> = Vec::new();
        // First policy Block reason encountered, if any. Inserted at the
        // front of `plugins` after the loop so it dominates the chain
        // regardless of the restricted entry's original position (mirrors the
        // HTTP "gate first" semantics).
        let mut policy_block_reason: Option<String> = None;

        for (index, entry) in entries.iter().enumerate() {
            if !entry.is_enabled() {
                continue;
            }

            // Policy is selected by the admitted Gateway scope before entry
            // construction. Stream admission has no HTTP response status.
            let action = policy.and_then(|policy| policy.action_for(PluginFamily::TlsRoute, entry.type_name()));
            match action {
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { .. }) => {
                    if policy_block_reason.is_none() {
                        policy_block_reason =
                            Some(format!("plugin disabled by policy: tls-route/{}", entry.type_name()));
                    }
                    continue;
                }
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass) => {
                    continue;
                }
                None => {}
            }

            let plugin = cache[index].get_or_insert_with(|| match &entry.plugin {
                TlsRouteStreamPlugin::IpRestriction(config) => {
                    Arc::new(TlsRouteIpRestriction::new(config, parent_namespace))
                }
            });

            plugins.push(Arc::clone(plugin));
        }

        if let Some(reason) = policy_block_reason {
            plugins.insert(0, Arc::new(PolicyBlockTlsRoutePlugin { reason }));
        }

        Self { plugins }
    }

    /// Execute all plugins in order. Returns Deny on first deny, otherwise Allow.
    pub async fn run(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        for plugin in &self.plugins {
            match plugin.on_tls_route(ctx).await {
                StreamPluginResult::Allow => continue,
                StreamPluginResult::Deny(reason) => return StreamPluginResult::Deny(reason),
            }
        }
        StreamPluginResult::Allow
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

/// Always-deny placeholder for a policy-blocked TLS route plugin entry.
struct PolicyBlockTlsRoutePlugin {
    reason: String,
}

#[async_trait::async_trait]
impl TlsRoutePlugin for PolicyBlockTlsRoutePlugin {
    async fn on_tls_route(&self, _ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        StreamPluginResult::Deny(self.reason.clone())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/tls_route/tls_route_plugin_runtime_test.rs"]
mod tests;

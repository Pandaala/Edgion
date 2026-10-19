//! Runtime for executing TLS route plugins in order (first-deny-wins).

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::stream::stream_plugin_trait::StreamPluginResult;
use edgion_resources::refs::ip_list_fields::{IpListSide, StreamIpListBindings};
use edgion_resources::refs::ReferenceError;
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::resources::edgion_stream_plugins::TlsRouteStreamPlugin;
use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily};
use std::borrow::Cow;
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

    /// Reject every attachment of a source containing an unsupported type.
    pub(crate) fn unsupported_source() -> Self {
        Self {
            plugins: vec![Arc::new(FailClosedTlsRoutePlugin {
                reason: "unsupported stream plugin type".into(),
            })],
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_plugins(plugins: Vec<Arc<dyn TlsRoutePlugin>>) -> Self {
        Self { plugins }
    }

    pub(crate) fn from_owner_cached(
        binding: &StreamIpListBindings<'_>,
        policy: Option<&CompiledPluginPolicy>,
        cache: &mut [Option<Arc<dyn TlsRoutePlugin>>],
    ) -> Self {
        let entries = binding.owner().spec.tls_route_plugins.as_deref().unwrap_or_default();
        let mut plugins: Vec<Arc<dyn TlsRoutePlugin>> = Vec::new();
        // First policy Block reason encountered, if any. Inserted at the
        // front of `plugins` after the loop so it dominates the chain
        // regardless of the restricted entry's original position (mirrors the
        // HTTP "gate first" semantics).
        let mut policy_block_reason: Option<Cow<'static, str>> = None;
        let mut invalid_config_reason: Option<Cow<'static, str>> = None;

        for (index, entry) in entries.iter().enumerate() {
            if matches!(entry.plugin, TlsRouteStreamPlugin::Unsupported) {
                invalid_config_reason.get_or_insert_with(|| "unsupported TLS route plugin type".into());
                continue;
            }
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
                            Some(format!("plugin disabled by policy: tls-route/{}", entry.type_name()).into());
                    }
                    continue;
                }
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass) => {
                    continue;
                }
                None => {}
            }

            let valid = binding.tls_entry(index).is_some_and(|entry| {
                let refs = entry.references();
                entry.config().validation_error().is_none()
                    && refs.validate_owner().is_ok()
                    && refs.read_scopes(IpListSide::Allow).is_ok()
                    && refs.read_scopes(IpListSide::Deny).is_ok()
            });
            if !valid {
                cache[index] = None;
                invalid_config_reason.get_or_insert_with(|| "invalid owning IP reference".into());
                continue;
            }

            let plugin = cache[index].get_or_insert_with(|| match &entry.plugin {
                TlsRouteStreamPlugin::Unsupported => Arc::new(FailClosedTlsRoutePlugin {
                    reason: "unsupported TLS route plugin type".into(),
                }),
                TlsRouteStreamPlugin::IpRestriction(_) => match binding
                    .tls_entry(index)
                    .ok_or(ReferenceError::StaleBinding)
                    .and_then(TlsRouteIpRestriction::from_bound)
                {
                    Ok(plugin) => Arc::new(plugin),
                    Err(_) => Arc::new(FailClosedTlsRoutePlugin {
                        reason: "invalid owning IP reference".into(),
                    }),
                },
            });

            plugins.push(Arc::clone(plugin));
        }

        if let Some(reason) = policy_block_reason.or(invalid_config_reason) {
            plugins.insert(0, Arc::new(FailClosedTlsRoutePlugin { reason }));
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

/// Always-deny gate for policy blocks and unsupported source configurations.
struct FailClosedTlsRoutePlugin {
    reason: Cow<'static, str>,
}

#[async_trait::async_trait]
impl TlsRoutePlugin for FailClosedTlsRoutePlugin {
    async fn on_tls_route(&self, _ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        StreamPluginResult::Deny(self.reason.clone())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/tls_route/tls_route_plugin_runtime_test.rs"]
mod tests;

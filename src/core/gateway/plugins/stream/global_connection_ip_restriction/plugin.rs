//! Stream plugin that resolves a PM reference at request time and applies its rules.

use async_trait::async_trait;

use crate::core::gateway::plugins::runtime::plugin_metadata::get_global_plugin_metadata_store;
use crate::core::gateway::plugins::stream::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::types::resources::edgion_gateway_config::PluginReference;

const PLUGIN_NAME: &str = "GlobalConnectionIpRestriction";

pub struct GlobalConnectionIpRestrictionPlugin {
    plugin_metadata_ref: PluginReference,
    parent_namespace: String,
}

impl GlobalConnectionIpRestrictionPlugin {
    pub fn new(plugin_metadata_ref: PluginReference, parent_namespace: String) -> Self {
        Self {
            plugin_metadata_ref,
            parent_namespace,
        }
    }

    /// Resolve effective namespace: explicit ref.namespace > parent fallback.
    fn ref_namespace(&self) -> &str {
        self.plugin_metadata_ref
            .namespace
            .as_deref()
            .unwrap_or(&self.parent_namespace)
    }
}

#[async_trait]
impl StreamPlugin for GlobalConnectionIpRestrictionPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        let store = get_global_plugin_metadata_store();
        // Getter signature: (namespace, name)
        let Some(pm) = store.get_global_connection_ip_restriction(self.ref_namespace(), &self.plugin_metadata_ref.name)
        else {
            tracing::debug!(
                ref_ns = self.ref_namespace(),
                ref_name = %self.plugin_metadata_ref.name,
                "GlobalConnectionIpRestriction PM not found, fail-open",
            );
            return StreamPluginResult::Allow;
        };

        if !pm.enable {
            return StreamPluginResult::Allow;
        }

        let Some(profile) = pm.profiles.get(&pm.active_profile) else {
            tracing::warn!(
                ref_ns = self.ref_namespace(),
                ref_name = %self.plugin_metadata_ref.name,
                active_profile = %pm.active_profile,
                "Active profile not found in PM, fail-open",
            );
            return StreamPluginResult::Allow;
        };

        if profile.check_ip(&ctx.client_ip) {
            StreamPluginResult::Allow
        } else {
            // Future: structured access log. For now, trace-level only.
            tracing::trace!(
                ip = %ctx.client_ip,
                port = ctx.listener_port,
                ref_ns = self.ref_namespace(),
                ref_name = %self.plugin_metadata_ref.name,
                profile = %pm.active_profile,
                "Connection rejected by GlobalConnectionIpRestriction",
            );
            StreamPluginResult::Deny("blocked by global connection ip restriction".to_string())
        }
    }
}

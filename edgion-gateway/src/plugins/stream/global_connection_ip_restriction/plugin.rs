//! Stream plugin that resolves a PM reference at request time and applies its rules.

use async_trait::async_trait;

use crate::observe::metrics::record_conn_ip_restriction_event;
use crate::plugins::runtime::plugin_metadata::get_global_plugin_metadata_store;
use crate::plugins::stream::stream_plugin_trait::{
    access_decision_to_stream_result, StreamContext, StreamPlugin, StreamPluginResult,
};
use edgion_resources::resources::edgion_gateway_config::PluginReference;

const PLUGIN_NAME: &str = "GlobalConnectionIpRestriction";

pub struct GlobalConnectionIpRestrictionPlugin {
    /// Precomputed PM store key ("<effective_ns>/<name>"). Built once at
    /// construction so the per-connection on_connection path can avoid the
    /// format!() allocation inside the by-(ns,name) getter.
    metadata_key: String,
}

impl GlobalConnectionIpRestrictionPlugin {
    pub fn new(plugin_metadata_ref: PluginReference, parent_namespace: String) -> Self {
        let effective_ns = plugin_metadata_ref.namespace.as_deref().unwrap_or(&parent_namespace);
        let metadata_key = format!("{}/{}", effective_ns, plugin_metadata_ref.name);
        Self { metadata_key }
    }
}

#[async_trait]
impl StreamPlugin for GlobalConnectionIpRestrictionPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        let store = get_global_plugin_metadata_store();
        let Some(pm) = store.get_global_connection_ip_restriction_by_key(&self.metadata_key) else {
            // A configured PM reference that does not resolve is a real
            // misconfiguration (sync is gated by ConfigClient::is_ready before
            // listeners bind) — fail closed, consistent with the L4 ESP paths.
            record_conn_ip_restriction_event("fail_closed_pm_missing");
            return StreamPluginResult::Deny("ip:deny:pm-missing".to_string());
        };

        if !pm.enable {
            return StreamPluginResult::Allow;
        }

        let Some(profile) = pm.profiles.get(&pm.active_profile) else {
            // active_profile not present in the PM is an internal integrity
            // error — always fail closed.
            record_conn_ip_restriction_event("fail_closed_profile_missing");
            return StreamPluginResult::Deny("ip:deny:profile-missing".to_string());
        };

        access_decision_to_stream_result(profile.check_ip(&ctx.client_ip), || {
            record_conn_ip_restriction_event("denied")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_pm_denies_connection() {
        // The global PM store is process-wide and shared across tests, so use a
        // collision-proof name that nothing else registers.
        let plugin = GlobalConnectionIpRestrictionPlugin::new(
            PluginReference {
                name: "gcir-unit-test-missing-pm-do-not-register".to_string(),
                namespace: Some("default".to_string()),
            },
            "default".to_string(),
        );
        let ctx = StreamContext::new("10.0.0.1".parse().unwrap(), 8080);
        assert!(
            matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)),
            "a missing referenced PluginMetaData must deny the connection"
        );
    }
}

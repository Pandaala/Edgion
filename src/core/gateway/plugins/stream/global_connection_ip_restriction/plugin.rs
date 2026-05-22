//! Stream plugin that resolves a PM reference at request time and applies its rules.

use async_trait::async_trait;

use crate::core::gateway::observe::metrics::record_conn_ip_restriction_event;
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
        use crate::types::resources::edgion_plugins::{AccessDecision, DefaultAction};

        let store = get_global_plugin_metadata_store();
        // Getter signature: (namespace, name)
        let Some(pm) = store.get_global_connection_ip_restriction(self.ref_namespace(), &self.plugin_metadata_ref.name)
        else {
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

        match profile.check_ip(&ctx.client_ip) {
            AccessDecision::AllowByGroup(_) => StreamPluginResult::Allow,
            AccessDecision::DenyByGroup(g) => {
                record_conn_ip_restriction_event("denied");
                StreamPluginResult::Deny(format!("ip:deny:group={}", g))
            }
            AccessDecision::DenyByWhitelistMiss => {
                record_conn_ip_restriction_event("denied");
                StreamPluginResult::Deny("ip:deny:not-in-allow".to_string())
            }
            AccessDecision::Default(DefaultAction::Allow) => StreamPluginResult::Allow,
            AccessDecision::Default(DefaultAction::Deny) => {
                record_conn_ip_restriction_event("denied");
                StreamPluginResult::Deny("ip:deny:default".to_string())
            }
        }
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

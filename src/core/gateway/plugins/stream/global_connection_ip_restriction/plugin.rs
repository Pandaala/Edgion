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
            record_conn_ip_restriction_event("fail_open_pm_missing");
            return StreamPluginResult::Allow;
        };

        if !pm.enable {
            return StreamPluginResult::Allow;
        }

        let Some(profile) = pm.profiles.get(&pm.active_profile) else {
            record_conn_ip_restriction_event("fail_open_profile_missing");
            return StreamPluginResult::Allow;
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

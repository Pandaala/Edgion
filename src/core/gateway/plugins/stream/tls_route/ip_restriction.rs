//! IP Restriction plugin for TLS route stage (Stage 2, post-handshake).
//!
//! Uses the HTTP-layer `IpRestrictionConfig` (with protocol-level `message`/`status` support),
//! distinct from the L4 `StreamIpRestriction` which uses `ConnectionIpRestrictionRules`.
//! Applied post-TLS handshake, so it has access to SNI-routed context.

use async_trait::async_trait;

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::core::gateway::plugins::stream::StreamPluginResult;
use crate::types::resources::edgion_plugins::{AccessDecision, DefaultAction, IpRestrictionConfig};

const PLUGIN_NAME: &str = "TlsRouteIpRestriction";

pub struct TlsRouteIpRestriction {
    config: IpRestrictionConfig,
    init_failed: bool,
}

impl TlsRouteIpRestriction {
    pub fn new(config: &IpRestrictionConfig) -> Self {
        let mut config = config.clone();
        let init_failed = if let Err(e) = config.validate_and_init() {
            tracing::error!(
                error = %e,
                label = "TLS route plugin",
                "Failed to initialize IpRestrictionConfig; all connections will be denied",
            );
            true
        } else {
            false
        };
        Self { config, init_failed }
    }
}

#[async_trait]
impl TlsRoutePlugin for TlsRouteIpRestriction {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_tls_route(&self, ctx: &TlsRouteContext) -> StreamPluginResult {
        if self.init_failed {
            return StreamPluginResult::Deny("ip init fail".to_string());
        }
        match self.config.check_ip_access(&ctx.client_ip) {
            AccessDecision::AllowByGroup(_) => StreamPluginResult::Allow,
            AccessDecision::DenyByGroup(g) => StreamPluginResult::Deny(format!("ip:deny:group={}", g)),
            AccessDecision::DenyByWhitelistMiss => StreamPluginResult::Deny("ip:deny:not-in-allow".to_string()),
            AccessDecision::Default(DefaultAction::Allow) => StreamPluginResult::Allow,
            AccessDecision::Default(DefaultAction::Deny) => StreamPluginResult::Deny("ip:deny:default".to_string()),
        }
    }
}

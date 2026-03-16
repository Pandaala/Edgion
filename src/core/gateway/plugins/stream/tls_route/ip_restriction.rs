//! IP Restriction plugin for TLS route stage.
//!
//! Same logic as StreamIpRestriction but implements the TlsRoutePlugin trait,
//! giving it access to TlsRouteContext (SNI, route info, etc.).
//! The IP check itself is identical — delegating to IpRestrictionConfig::check_ip_access.

use async_trait::async_trait;

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::core::gateway::plugins::stream::StreamPluginResult;
use crate::types::resources::edgion_plugins::IpRestrictionConfig;

pub struct TlsRouteIpRestriction {
    name: String,
    config: IpRestrictionConfig,
}

impl TlsRouteIpRestriction {
    pub fn new(config: &IpRestrictionConfig) -> Self {
        let mut config = config.clone();
        if let Err(e) = config.validate_and_init() {
            tracing::error!(error = %e, "Failed to initialize IpRestrictionConfig for TLS route plugin");
        }
        Self {
            name: "TlsRouteIpRestriction".to_string(),
            config,
        }
    }
}

#[async_trait]
impl TlsRoutePlugin for TlsRouteIpRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_tls_route(&self, ctx: &TlsRouteContext) -> StreamPluginResult {
        if !self.config.check_ip_access(&ctx.client_ip) {
            let reason = self
                .config
                .message
                .as_deref()
                .unwrap_or("IP address not allowed")
                .to_string();
            return StreamPluginResult::Deny(reason);
        }
        StreamPluginResult::Allow
    }
}

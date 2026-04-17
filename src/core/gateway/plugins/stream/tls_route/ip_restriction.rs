//! IP Restriction plugin for TLS route stage.
//!
//! Same logic as StreamIpRestriction but implements the TlsRoutePlugin trait,
//! giving it access to TlsRouteContext (SNI, route info, etc.).
//! The IP check itself is identical — delegating to IpRestrictionConfig::check_ip_access.

use async_trait::async_trait;

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::core::gateway::plugins::stream::StreamPluginResult;
use crate::types::resources::edgion_plugins::IpRestrictionConfig;

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
            return StreamPluginResult::Deny("IP restriction plugin initialization failed".to_string());
        }
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

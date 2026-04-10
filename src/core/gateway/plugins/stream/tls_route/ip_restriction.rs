//! IP Restriction plugin for TLS route stage.
//!
//! Same logic as StreamIpRestriction but implements the TlsRoutePlugin trait,
//! giving it access to TlsRouteContext (SNI, route info, etc.).
//! The IP check itself is identical — delegating to IpRestrictionConfig::check_ip_access.

use async_trait::async_trait;

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::core::gateway::plugins::stream::ip_restriction::stream_ip_restriction::{
    check_ip_access, init_ip_restriction,
};
use crate::core::gateway::plugins::stream::StreamPluginResult;
use crate::types::resources::edgion_plugins::IpRestrictionConfig;

pub struct TlsRouteIpRestriction {
    name: String,
    config: IpRestrictionConfig,
    init_failed: bool,
}

impl TlsRouteIpRestriction {
    pub fn new(config: &IpRestrictionConfig) -> Self {
        let (config, init_failed) = init_ip_restriction(config, "TLS route plugin");
        Self {
            name: "TlsRouteIpRestriction".to_string(),
            config,
            init_failed,
        }
    }
}

#[async_trait]
impl TlsRoutePlugin for TlsRouteIpRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_tls_route(&self, ctx: &TlsRouteContext) -> StreamPluginResult {
        check_ip_access(&self.config, self.init_failed, &ctx.client_ip)
    }
}

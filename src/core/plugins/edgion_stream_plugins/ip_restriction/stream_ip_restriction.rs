//! IP Restriction plugin for stream connections (TCP/UDP)
//!
//! This plugin controls access to TCP/UDP connections based on client IP addresses.
//!
//! ## Access Control Rules:
//!
//! 1. **Deny has highest priority**: If IP matches deny list, connection is rejected
//! 2. **Allow checked next**: If IP matches allow list, connection is accepted
//! 3. **Default action applies**: If no rules match, use the configured default action
//!
//! ## Differences from HTTP IP Restriction:
//!
//! - Operates at connection level (before any data exchange)
//! - No HTTP response needed - connection is simply closed
//! - More efficient for TCP/UDP protocols
//! - Uses same IpRestrictionConfig for consistency

use async_trait::async_trait;

use crate::core::plugins::edgion_stream_plugins::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::types::resources::edgion_plugins::IpRestrictionConfig;

/// Stream IP Restriction plugin
pub struct StreamIpRestriction {
    name: String,
    config: IpRestrictionConfig,
}

impl StreamIpRestriction {
    /// Create a new StreamIpRestriction plugin from configuration
    pub fn new(config: &IpRestrictionConfig) -> Self {
        let mut config = config.clone();
        // Initialize runtime matchers (allow_matcher, deny_matcher) from allow/deny lists.
        // These are #[serde(skip)] fields that must be built after deserialization.
        if let Err(e) = config.validate_and_init() {
            tracing::error!(error = %e, "Failed to initialize IpRestrictionConfig for stream plugin");
        }
        Self {
            name: "StreamIpRestriction".to_string(),
            config,
        }
    }
}

#[async_trait]
impl StreamPlugin for StreamIpRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        let client_ip = ctx.client_ip;

        tracing::debug!(
            plugin = self.name,
            client_ip = %client_ip,
            listener_port = ctx.listener_port,
            "Checking IP restriction for stream connection"
        );

        // Check access using config's check_ip_access method
        if !self.config.check_ip_access(&client_ip) {
            let reason = self
                .config
                .message
                .as_deref()
                .unwrap_or("IP address not allowed")
                .to_string();

            tracing::info!(
                plugin = self.name,
                client_ip = %client_ip,
                "Access DENIED for stream connection"
            );

            return StreamPluginResult::Deny(reason);
        }

        tracing::debug!(
            plugin = self.name,
            client_ip = %client_ip,
            "Access ALLOWED for stream connection"
        );

        StreamPluginResult::Allow
    }
}

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
//!
//! NOTE: Task 10 will rewire this to use `ConnectionIpRestrictionRules` instead of
//! `IpRestrictionConfig`. Dead-code allowed until then.
#![allow(dead_code)]

use async_trait::async_trait;

use crate::core::gateway::plugins::stream::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::types::resources::edgion_plugins::IpRestrictionConfig;

use std::net::IpAddr;

/// Initialize an `IpRestrictionConfig` with fail-closed semantics.
/// Returns `(config, init_failed)`.
pub(crate) fn init_ip_restriction(config: &IpRestrictionConfig, label: &str) -> (IpRestrictionConfig, bool) {
    let mut config = config.clone();
    let init_failed = if let Err(e) = config.validate_and_init() {
        tracing::error!(error = %e, "Failed to initialize IpRestrictionConfig for {}, all connections will be denied", label);
        true
    } else {
        false
    };
    (config, init_failed)
}

/// Check IP access with fail-closed guard.
pub(crate) fn check_ip_access(config: &IpRestrictionConfig, init_failed: bool, ip: &IpAddr) -> StreamPluginResult {
    if init_failed {
        return StreamPluginResult::Deny("IP restriction plugin initialization failed".to_string());
    }
    if !config.check_ip_access(ip) {
        let reason = config
            .message
            .as_deref()
            .unwrap_or("IP address not allowed")
            .to_string();
        return StreamPluginResult::Deny(reason);
    }
    StreamPluginResult::Allow
}

/// Stream IP Restriction plugin
pub struct StreamIpRestriction {
    name: String,
    config: IpRestrictionConfig,
    init_failed: bool,
}

impl StreamIpRestriction {
    pub fn new(config: &IpRestrictionConfig) -> Self {
        let (config, init_failed) = init_ip_restriction(config, "stream plugin");
        Self {
            name: "StreamIpRestriction".to_string(),
            config,
            init_failed,
        }
    }
}

#[async_trait]
impl StreamPlugin for StreamIpRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        check_ip_access(&self.config, self.init_failed, &ctx.client_ip)
    }
}

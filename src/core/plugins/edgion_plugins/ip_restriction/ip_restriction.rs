//! IP Restriction plugin implementation
//!
//! This plugin controls access based on client IP addresses using allow/deny lists.
//!
//! ## Access Control Rules (nginx-compatible):
//!
//! 1. **Deny has highest priority**: If IP matches deny list, access is denied immediately
//! 2. **Allow checked next**: If IP matches allow list, access is granted
//! 3. **Default action applies**: If no rules match, use the configured default action
//!
//! ## Configuration Examples:
//!
//! ### Whitelist only specific network:
//! ```yaml
//! ipRestriction:
//!   allow: ["192.168.1.0/24"]
//! ```
//!
//! ### Blacklist specific IPs:
//! ```yaml
//! ipRestriction:
//!   deny: ["192.168.1.100", "10.0.0.50"]
//! ```
//!
//! ### Combined: Allow subnet but deny specific IP:
//! ```yaml
//! ipRestriction:
//!   allow: ["10.0.0.0/8"]
//!   deny: ["10.0.0.100"]
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use std::net::IpAddr;

use crate::core::plugins::{Plugin, PluginSession, PluginLog};
use crate::types::filters::{PluginConf, PluginRunningResult, PluginRunningStage};
use crate::types::resources::edgion_plugins::{IpRestrictionConfig, IpSource};

/// IP Restriction plugin
pub struct IpRestriction {
    name: String,
    config: IpRestrictionConfig,
    stages: Vec<PluginRunningStage>,
}

impl IpRestriction {
    /// Create a new IpRestriction plugin from configuration
    pub fn new(config: &IpRestrictionConfig) -> Box<dyn Plugin> {
        let ip_restriction = IpRestriction {
            name: "IpRestriction".to_string(),
            config: config.clone(),
            stages: vec![PluginRunningStage::Request],
        };

        Box::new(ip_restriction)
    }

    /// Extract client IP from session based on configured source
    fn get_client_ip(&self, session: &mut dyn PluginSession) -> Option<IpAddr> {
        let ip_str = match self.config.ip_source {
            IpSource::ClientIp => session.remote_addr(),  // Real client IP from proxy headers
            IpSource::RemoteAddr => session.client_addr(), // Direct TCP connection address
        };

        // Return None if empty string (not yet populated)
        if ip_str.is_empty() {
            return None;
        }

        ip_str.parse::<IpAddr>().ok()
    }
}

#[async_trait]
impl Plugin for IpRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_async(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        if stage != PluginRunningStage::Request {
            return PluginRunningResult::GoodNext;
        }

        // Get client IP
        let client_ip = match self.get_client_ip(session) {
            Some(ip) => ip,
            None => {
                plugin_log.add_plugin_log("Failed to get client IP\n");
                // If we can't determine IP, allow by default (or could be configured)
                return PluginRunningResult::GoodNext;
            }
        };

        plugin_log.add_plugin_log(&format!("Client IP: {}\n", client_ip));

        // Check access using config's check_ip_access method
        if !self.config.check_ip_access(&client_ip) {
            let message = self.config.message.as_deref()
                .unwrap_or("Your IP address is not allowed to access this resource");

            plugin_log.add_plugin_log(&format!("Access DENIED for {}\n", client_ip));

            // Build error response
            let mut resp = Box::new(ResponseHeader::build(self.config.status, None).unwrap());
            resp.insert_header("Content-Type", "application/json").ok();

            let body = Bytes::from(format!(r#"{{"message":"{}"}}"#, message));

            // Write response
            if let Err(e) = session.write_response_header(resp, false).await {
                plugin_log.add_plugin_log(&format!("Failed to write response header: {}\n", e));
                return PluginRunningResult::ErrTerminateRequest;
            }

            if let Err(e) = session.write_response_body(Some(body), true).await {
                plugin_log.add_plugin_log(&format!("Failed to write response body: {}\n", e));
                return PluginRunningResult::ErrTerminateRequest;
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        plugin_log.add_plugin_log(&format!("Access ALLOWED for {}\n", client_ip));
        PluginRunningResult::GoodNext
    }

    fn get_stages(&self) -> Vec<PluginRunningStage> {
        self.stages.clone()
    }

    fn check_schema(&self, _conf: &PluginConf) {
        // Schema validation is done in IpRestrictionConfig::new
    }
}


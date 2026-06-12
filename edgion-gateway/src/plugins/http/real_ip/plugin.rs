//! Real IP plugin implementation
//!
//! Extracts the real client IP address from HTTP headers when behind trusted proxies.
//! Reuses the global RealIpExtractor for consistent behavior.

use async_trait::async_trait;
use std::net::{IpAddr, SocketAddr};

use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_common::utils::RealIpExtractor;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::RealIpConfig;

/// RealIp plugin for extracting real client IP
///
/// Wraps the global RealIpExtractor for use in plugin chain.
pub struct RealIp {
    name: String,
    extractor: Option<RealIpExtractor>,
}

impl RealIp {
    /// Create a new RealIp plugin from configuration
    pub fn create(config: &RealIpConfig) -> Box<dyn RequestFilter> {
        // Build RealIpExtractor from plugin config
        let extractor = if !config.trusted_ips.is_empty() {
            match RealIpExtractor::new(
                &config.trusted_ips,
                config.real_ip_header.clone(),
                config.recursive,
                config.max_trusted_hops,
            ) {
                Ok(e) => Some(e),
                Err(err) => {
                    tracing::error!(
                        error = ?err,
                        "Failed to create RealIpExtractor from plugin config"
                    );
                    None
                }
            }
        } else {
            tracing::warn!("RealIp plugin: trusted_ips is empty, plugin will be no-op");
            None
        };

        let plugin = RealIp {
            name: "RealIp".to_string(),
            extractor,
        };

        Box::new(plugin)
    }
}

#[async_trait]
impl RequestFilter for RealIp {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Check if extractor is available
        let Some(ref extractor) = self.extractor else {
            plugin_log.push_err("no extractor");
            return PluginRunningResult::GoodNext;
        };

        // Push which header is configured
        match extractor.real_ip_header().to_ascii_lowercase().as_str() {
            "x-forwarded-for" => plugin_log.push("header xff"),
            "x-real-ip" => plugin_log.push("header real-ip"),
            "forwarded" => plugin_log.push("header forwarded"),
            _ => plugin_log.push("header custom"),
        };

        // Check if client addr is a trusted proxy
        let client_addr_str = session.client_addr();
        let client_ip: IpAddr = client_addr_str
            .parse::<SocketAddr>()
            .map(|sa| sa.ip())
            .or_else(|_| client_addr_str.parse::<IpAddr>())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

        if extractor.is_trusted_proxy(&client_ip) {
            plugin_log.push("trusted proxy:match");
        } else {
            plugin_log.push("trusted proxy:miss");
        }

        // Get current remote_addr (clone to avoid borrow conflicts)
        let old_remote_addr = session.remote_addr().to_string();

        // Extract new real IP using RealIpExtractor
        let new_remote_addr = if let Some(header_value) = session.header_value(extractor.real_ip_header()) {
            extractor.extract_from_header_value(session.client_addr(), &header_value)
        } else {
            session.client_addr().to_string()
        };

        // Update remote_addr if it changed
        if new_remote_addr != old_remote_addr {
            if let Err(_e) = session.set_remote_addr(&new_remote_addr) {
                plugin_log.push_err("set remote addr failed");
                return PluginRunningResult::GoodNext;
            }

            plugin_log.push_fmt("ip updated: {}", la![sensitive(&new_remote_addr)]);
        } else {
            plugin_log.push("ip unchanged");
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> RealIpConfig {
        use edgion_resources::resources::plugin_metadata::IpGroup;
        RealIpConfig {
            trusted_ips: vec![IpGroup {
                name: "private".to_string(),
                description: None,
                cidrs: vec!["192.168.0.0/16".to_string(), "198.51.100.0/24".to_string()],
            }],
            real_ip_header: "X-Forwarded-For".to_string(),
            recursive: true,
            max_trusted_hops: None,
        }
    }

    #[test]
    fn test_extract_recursive() {
        let config = create_test_config(); // recursive: true
        let extractor = RealIpExtractor::new(
            &config.trusted_ips,
            config.real_ip_header.clone(),
            config.recursive,
            config.max_trusted_hops,
        )
        .unwrap();

        // Case 1: Right-to-left, find first non-trusted
        let result = extractor.extract_from_header_value("192.168.1.254", "203.0.113.1, 198.51.100.2, 192.168.1.1");
        assert_eq!(result, "203.0.113.1");

        // Case 2: All trusted, use leftmost (Nginx behavior)
        let result = extractor.extract_from_header_value("192.168.1.254", "192.168.1.1, 198.51.100.2");
        assert_eq!(result, "192.168.1.1");

        // Case 3: Single untrusted IP
        let result = extractor.extract_from_header_value("192.168.1.254", "8.8.8.8");
        assert_eq!(result, "8.8.8.8");
    }

    #[test]
    fn test_client_not_trusted() {
        let config = create_test_config();
        let extractor = RealIpExtractor::new(
            &config.trusted_ips,
            config.real_ip_header.clone(),
            config.recursive,
            config.max_trusted_hops,
        )
        .unwrap();

        // Client IP not in trusted list - should use client_addr directly
        let result = extractor.extract_from_header_value("8.8.8.8", "203.0.113.1");
        assert_eq!(result, "8.8.8.8");
    }
}

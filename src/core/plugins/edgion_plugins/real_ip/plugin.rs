//! Real IP plugin implementation
//!
//! Extracts the real client IP address from HTTP headers when behind trusted proxies.
//! Reuses the global RealIpExtractor for consistent behavior.

use async_trait::async_trait;
use tracing::debug;

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::core::utils::RealIpExtractor;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::RealIpConfig;

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
            match RealIpExtractor::new(&config.trusted_ips, config.real_ip_header.clone()) {
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
            plugin_log.push("Config error: No valid extractor; ");
            return PluginRunningResult::GoodNext;
        };

        // Get current remote_addr (clone to avoid borrow conflicts)
        let old_remote_addr = session.remote_addr().to_string();

        // Extract new real IP using RealIpExtractor

        // Build a minimal RequestHeader to pass to extractor
        // We need to get the header value and pass it through
        let new_remote_addr = if let Some(header_value) = session.header_value(extractor.real_ip_header()) {
            // Use extractor's internal logic via a simpler approach
            // Since we can't easily construct RequestHeader, extract the IP directly
            extract_real_ip_with_extractor(extractor, session.client_addr(), &header_value)
        } else {
            // No header found, check if client is trusted
            if let Ok(client_ip) = session.client_addr().parse() {
                if extractor.is_trusted_proxy(&client_ip) {
                    // Client is trusted but no header, fallback
                    session.client_addr().to_string()
                } else {
                    // Client not trusted, use it directly
                    session.client_addr().to_string()
                }
            } else {
                session.client_addr().to_string()
            }
        };

        // Update remote_addr if it changed
        if new_remote_addr != old_remote_addr {
            if let Err(e) = session.set_remote_addr(&new_remote_addr) {
                plugin_log.push(&format!("Failed to set remote_addr: {}; ", e));
                return PluginRunningResult::GoodNext;
            }

            debug!(
                old = %old_remote_addr,
                new = %new_remote_addr,
                "RealIp: Updated remote_addr"
            );
            plugin_log.push(&format!(
                "Extracted real IP: {} (was: {}); ",
                new_remote_addr, old_remote_addr
            ));
        } else {
            plugin_log.push(&format!("Real IP unchanged: {}; ", new_remote_addr));
        }

        PluginRunningResult::GoodNext
    }
}

/// Helper function to extract real IP using RealIpExtractor logic
fn extract_real_ip_with_extractor(extractor: &RealIpExtractor, client_addr: &str, header_value: &str) -> String {
    use std::net::IpAddr;

    // Parse client_addr
    let client_ip = match client_addr.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(_) => return client_addr.to_string(),
    };

    // If client not trusted, use it directly
    if !extractor.is_trusted_proxy(&client_ip) {
        return client_ip.to_string();
    }

    // Parse header value (comma-separated IPs)
    let ips: Vec<&str> = header_value
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    // Traverse from right to left (Nginx style, always recursive for now)
    for ip_str in ips.iter().rev() {
        if let Ok(ip_addr) = ip_str.parse::<IpAddr>() {
            if !extractor.is_trusted_proxy(&ip_addr) {
                return ip_addr.to_string();
            }
        }
    }

    // All trusted or no valid IP, return leftmost or client
    ips.first()
        .and_then(|s| s.parse::<IpAddr>().ok())
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| client_ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;

    fn create_test_config() -> RealIpConfig {
        RealIpConfig {
            trusted_ips: vec!["192.168.0.0/16".to_string(), "198.51.100.0/24".to_string()],
            real_ip_header: "X-Forwarded-For".to_string(),
            recursive: true,
            trusted_ip_matcher: None, // Will be initialized by RealIp::create
        }
    }

    #[test]
    fn test_extract_real_ip_with_extractor_recursive() {
        let config = create_test_config();
        let extractor = RealIpExtractor::new(&config.trusted_ips, config.real_ip_header.clone()).unwrap();

        // Case 1: Right-to-left, find first non-trusted
        let header = "203.0.113.1, 198.51.100.2, 192.168.1.1";
        let result = extract_real_ip_with_extractor(&extractor, "192.168.1.254", header);
        assert_eq!(result, "203.0.113.1");

        // Case 2: All trusted, use leftmost
        let header = "192.168.1.1, 198.51.100.2";
        let result = extract_real_ip_with_extractor(&extractor, "192.168.1.254", header);
        assert_eq!(result, "192.168.1.1");

        // Case 3: Single untrusted IP
        let header = "8.8.8.8";
        let result = extract_real_ip_with_extractor(&extractor, "192.168.1.254", header);
        assert_eq!(result, "8.8.8.8");
    }

    #[test]
    fn test_client_not_trusted() {
        let config = create_test_config();
        let extractor = RealIpExtractor::new(&config.trusted_ips, config.real_ip_header.clone()).unwrap();

        // Client IP not in trusted list - should use client_addr directly
        let header = "203.0.113.1";
        let result = extract_real_ip_with_extractor(&extractor, "8.8.8.8", header);
        assert_eq!(result, "8.8.8.8");
    }

    #[tokio::test]
    async fn test_run_request() {
        let config = create_test_config();
        let plugin = RealIp::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RealIp");

        mock_session
            .expect_client_addr()
            .return_const("192.168.1.254".to_string());
        mock_session
            .expect_remote_addr()
            .return_const("192.168.1.254".to_string());
        mock_session
            .expect_header_value()
            .withf(|name| name == "X-Forwarded-For")
            .return_const(Some("203.0.113.1, 198.51.100.2, 192.168.1.1".to_string()));
        mock_session
            .expect_set_remote_addr()
            .withf(|addr| addr == "203.0.113.1")
            .returning(|_| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("203.0.113.1"));
    }
}

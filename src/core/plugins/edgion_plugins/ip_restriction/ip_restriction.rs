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

use crate::core::plugins::plugin_runtime::{RequestFilter, PluginSession, PluginLog};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{IpRestrictionConfig, IpSource};

/// IP Restriction plugin
pub struct IpRestriction {
    name: String,
    config: IpRestrictionConfig,
}

impl IpRestriction {
    /// Create a new IpRestriction plugin from configuration
    pub fn new(config: &IpRestrictionConfig) -> Box<dyn RequestFilter> {
        let ip_restriction = IpRestriction {
            name: "IpRestriction".to_string(),
            config: config.clone(),
        };

        Box::new(ip_restriction)
    }

    /// Extract client IP from session based on configured source
    fn get_client_ip(&self, session: &mut dyn PluginSession) -> Option<IpAddr> {
        let ip_str = match self.config.ip_source {
            IpSource::ClientIp => session.remote_addr(),  // Real client IP from proxy headers
            IpSource::RemoteAddr => session.client_addr(), // Direct TCP connection IP (without port)
        };

        // Return None if empty string (not yet populated)
        if ip_str.is_empty() {
            return None;
        }

        ip_str.parse::<IpAddr>().ok()
    }
}

#[async_trait]
impl RequestFilter for IpRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Get client IP
        let client_ip = match self.get_client_ip(session) {
            Some(ip) => ip,
            None => {
                plugin_log.push("Failed to get client IP\n");
                // If we can't determine IP, allow by default (or could be configured)
                return PluginRunningResult::GoodNext;
            }
        };

        plugin_log.push(&format!("Client IP: {}\n", client_ip));

        // Check access using config's check_ip_access method
        if !self.config.check_ip_access(&client_ip) {
            let message = self.config.message.as_deref()
                .unwrap_or("Your IP address is not allowed to access this resource");

            plugin_log.push(&format!("Access DENIED for {}\n", client_ip));

            // Build error response
            let mut resp = Box::new(ResponseHeader::build(self.config.status, None).unwrap());
            resp.insert_header("Content-Type", "application/json").ok();

            let body = Bytes::from(format!(r#"{{"message":"{}"}}"#, message));

            // Write response
            if let Err(e) = session.write_response_header(resp, false).await {
                plugin_log.push(&format!("Failed to write response header: {}\n", e));
                return PluginRunningResult::ErrTerminateRequest;
            }

            if let Err(e) = session.write_response_body(Some(body), true).await {
                plugin_log.push(&format!("Failed to write response body: {}\n", e));
                return PluginRunningResult::ErrTerminateRequest;
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        plugin_log.push(&format!("Access ALLOWED for {}\n", client_ip));
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::DefaultAction;

    fn create_ip_config_allow_list() -> IpRestrictionConfig {
        let config = IpRestrictionConfig {
            allow: Some(vec!["192.168.1.0/24".to_string()]),
            deny: None,
            default_action: DefaultAction::Deny,
            ip_source: IpSource::ClientIp,
            message: None,
            status: 403,
            allow_matcher: None,
            deny_matcher: None,
        };
        IpRestrictionConfig::new(config).unwrap()
    }

    fn create_ip_config_deny_list() -> IpRestrictionConfig {
        let config = IpRestrictionConfig {
            allow: None,
            deny: Some(vec!["10.0.0.100".to_string()]),
            default_action: DefaultAction::Allow,
            ip_source: IpSource::ClientIp,
            message: None,
            status: 403,
            allow_matcher: None,
            deny_matcher: None,
        };
        IpRestrictionConfig::new(config).unwrap()
    }

    #[tokio::test]
    async fn test_allowed_ip_passes() {
        let config = create_ip_config_allow_list();
        let plugin = IpRestriction::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        mock_session
            .expect_remote_addr()
            .return_const("192.168.1.50".to_string());

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("ALLOWED"));
    }

    #[tokio::test]
    async fn test_denied_ip_blocked() {
        let config = create_ip_config_allow_list();
        let plugin = IpRestriction::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        mock_session
            .expect_remote_addr()
            .return_const("10.0.0.50".to_string());
        mock_session
            .expect_write_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_write_response_body()
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.log.as_ref().unwrap().contains("DENIED"));
    }

    #[tokio::test]
    async fn test_blacklist_blocks_specific_ip() {
        let config = create_ip_config_deny_list();
        let plugin = IpRestriction::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        mock_session
            .expect_remote_addr()
            .return_const("10.0.0.100".to_string());
        mock_session
            .expect_write_response_header()
            .returning(|_, _| Ok(()));
        mock_session
            .expect_write_response_body()
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.log.as_ref().unwrap().contains("DENIED"));
    }

    #[tokio::test]
    async fn test_default_action_allow() {
        let config = create_ip_config_deny_list();
        let plugin = IpRestriction::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        mock_session
            .expect_remote_addr()
            .return_const("172.16.0.1".to_string());

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("ALLOWED"));
    }

    #[tokio::test]
    async fn test_invalid_ip_continues() {
        let config = create_ip_config_allow_list();
        let plugin = IpRestriction::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        mock_session
            .expect_remote_addr()
            .return_const("".to_string());

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.log.as_ref().unwrap().contains("Failed to get client IP"));
    }
}

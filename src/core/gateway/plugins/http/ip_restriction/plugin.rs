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

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{DefaultAction, IpRestrictionConfig, IpSource};

/// IP Restriction plugin
pub struct IpRestriction {
    name: String,
    config: IpRestrictionConfig,
}

impl IpRestriction {
    /// Create a new IpRestriction plugin from configuration
    pub fn create(config: &IpRestrictionConfig) -> Box<dyn RequestFilter> {
        let ip_restriction = IpRestriction {
            name: "IpRestriction".to_string(),
            config: config.clone(),
        };

        Box::new(ip_restriction)
    }

    /// Extract client IP from session based on configured source
    fn get_client_ip(&self, session: &mut dyn PluginSession) -> Option<IpAddr> {
        let ip_str = match self.config.ip_source {
            IpSource::ClientIp => session.remote_addr(), // Real client IP from proxy headers
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

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        use crate::types::resources::edgion_plugins::AccessDecision;

        let client_ip_opt = self.get_client_ip(session);

        let should_deny = match client_ip_opt {
            Some(ip) => match self.config.check_ip_access(&ip) {
                AccessDecision::AllowByGroup(g) => {
                    plugin_log.push(&format!("allow:group={}", g));
                    false
                }
                AccessDecision::DenyByGroup(g) => {
                    plugin_log.push(&format!("deny:group={}", g));
                    true
                }
                AccessDecision::DenyByWhitelistMiss => {
                    plugin_log.push("deny:not-in-allow");
                    true
                }
                AccessDecision::Default(DefaultAction::Allow) => {
                    plugin_log.push("allow:default");
                    false
                }
                AccessDecision::Default(DefaultAction::Deny) => {
                    plugin_log.push("deny:default");
                    true
                }
            },
            None => {
                // No client IP could be extracted.
                let whitelist_mode = self.config.allow_matcher.is_some();
                if whitelist_mode {
                    plugin_log.push("deny:no-client-ip");
                    true
                } else {
                    let deny = matches!(self.config.default_action, DefaultAction::Deny);
                    plugin_log.push(if deny {
                        "deny:no-client-ip"
                    } else {
                        "allow:no-client-ip"
                    });
                    deny
                }
            }
        };

        if should_deny {
            let message = self
                .config
                .message
                .as_deref()
                .unwrap_or("Your IP address is not allowed to access this resource");

            // Build error response
            let mut resp = Box::new(
                ResponseHeader::build(self.config.status, None)
                    .unwrap_or_else(|_| ResponseHeader::build(403, None).expect("403 is valid")),
            );
            resp.insert_header("Content-Type", "application/json").ok();

            let body = Bytes::from(serde_json::json!({"message": message}).to_string());

            if let Err(_e) = session.write_response_header(resp, false).await {
                plugin_log.push_err("write response header fail");
                return PluginRunningResult::ErrTerminateRequest;
            }

            if let Err(_e) = session.write_response_body(Some(body), true).await {
                plugin_log.push_err("write response body fail");
                return PluginRunningResult::ErrTerminateRequest;
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::DefaultAction;
    use crate::types::resources::plugin_metadata::IpGroup;

    fn create_ip_config_allow_list() -> IpRestrictionConfig {
        let config = IpRestrictionConfig {
            allow: Some(vec![IpGroup {
                name: "default".into(),
                description: None,
                cidrs: vec!["192.168.1.0/24".into()],
            }]),
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
            deny: Some(vec![IpGroup {
                name: "default".into(),
                description: None,
                cidrs: vec!["10.0.0.100".into()],
            }]),
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
    async fn test_default_action_allow() {
        let config = create_ip_config_deny_list();
        let plugin = IpRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        mock_session.expect_remote_addr().return_const("172.16.0.1".to_string());

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("allow:default"));
    }

    #[tokio::test]
    async fn test_missing_ip_denied_in_whitelist_mode() {
        // Allow list configured = whitelist mode: unknown IP must be denied
        let config = create_ip_config_allow_list();
        let plugin = IpRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        mock_session.expect_remote_addr().return_const("".to_string());
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("deny:no-client-ip"));
    }

    #[tokio::test]
    async fn test_whitelist_miss_emits_not_in_allow() {
        let config = create_ip_config_allow_list();
        let plugin = IpRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("IpRestriction");

        // An IP outside the configured allow range.
        mock_session.expect_remote_addr().return_const("10.0.0.1".to_string());
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("deny:not-in-allow"));
    }
}

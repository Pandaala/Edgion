//! Request Restriction plugin implementation
//!
//! This plugin restricts access based on request attributes like headers, cookies,
//! query parameters, path, method, and referer.
//!
//! ## Features:
//! - Support for Header, Cookie, Query, Path, Method, and Referer sources
//! - Exact match (allow/deny) and regex match (allowRegex/denyRegex)
//! - Automatic HashSet optimization for large value lists (>16)
//! - Allow/Deny lists with deny taking precedence
//! - Configurable missing value handling
//! - Multiple rules with Any/All combination modes
//!
//! ## Configuration Examples:
//!
//! ### Block bots by User-Agent (regex):
//! ```yaml
//! requestRestriction:
//!   rules:
//!     - name: "block-bots"
//!       source: Header
//!       key: "User-Agent"
//!       denyRegex:
//!         - "(?i).*Bot.*"
//!         - "(?i).*Spider.*"
//!       onMissing: Allow
//!   message: "Bot access denied"
//! ```
//!
//! ### Allow only specific paths:
//! ```yaml
//! requestRestriction:
//!   rules:
//!     - name: "api-only"
//!       source: Path
//!       allow: ["/health"]
//!       allowRegex: ["^/api/.*"]
//! ```
//!
//! ### Whitelist HTTP methods (exact):
//! ```yaml
//! requestRestriction:
//!   rules:
//!     - name: "readonly"
//!       source: Method
//!       allow: ["GET", "HEAD", "OPTIONS"]
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{
    RequestRestrictionConfig, RestrictionRule, RestrictionSource, RuleMatchMode,
};

/// Request Restriction plugin
pub struct RequestRestriction {
    name: String,
    config: RequestRestrictionConfig,
}

impl RequestRestriction {
    /// Create a new RequestRestriction plugin from configuration
    pub fn create(config: &RequestRestrictionConfig) -> Box<dyn RequestFilter> {
        // Clone and validate the config to compile regex patterns
        let mut validated_config = config.clone();
        validated_config.validate();

        let plugin = RequestRestriction {
            name: "RequestRestriction".to_string(),
            config: validated_config,
        };

        Box::new(plugin)
    }

    /// Extract value from session based on rule source and key
    fn get_value(&self, session: &dyn PluginSession, rule: &RestrictionRule) -> Option<String> {
        match rule.source {
            RestrictionSource::Header => {
                let key = rule.key.as_deref()?;
                session.header_value(key)
            }
            RestrictionSource::Cookie => {
                let key = rule.key.as_deref()?;
                session.get_cookie(key)
            }
            RestrictionSource::Query => {
                let key = rule.key.as_deref()?;
                session.get_query_param(key)
            }
            RestrictionSource::Path => Some(session.get_path().to_string()),
            RestrictionSource::Method => Some(session.get_method().to_string()),
            RestrictionSource::Referer => session.header_value("Referer"),
        }
    }

    /// Evaluate all rules and determine if request should be denied
    /// Returns: (denied: bool, rule_name: Option<String>)
    fn evaluate_rules(&self, session: &dyn PluginSession) -> (bool, Option<String>) {
        let mut denied_count = 0;
        let mut denied_rule_name = None;
        let total_rules = self.config.rules.len();

        for rule in &self.config.rules {
            let value = self.get_value(session, rule);
            let result = rule.check_value(value.as_deref());

            match result {
                Some(true) => {
                    // Rule triggered denial
                    denied_count += 1;
                    if denied_rule_name.is_none() {
                        denied_rule_name = Some(rule.display_name());
                    }

                    // For "Any" mode, we can short-circuit on first denial
                    if self.config.match_mode == RuleMatchMode::Any {
                        return (true, denied_rule_name);
                    }
                }
                Some(false) => {
                    // Rule allowed - for "All" mode, this means we won't deny
                    if self.config.match_mode == RuleMatchMode::All {
                        return (false, None);
                    }
                }
                None => {
                    // Rule skipped (OnMissing::Skip)
                    // Don't count this rule
                }
            }
        }

        // Final decision based on match mode
        match self.config.match_mode {
            RuleMatchMode::Any => {
                // If any rule denied, we already returned above
                // If we're here, no rule denied
                (false, None)
            }
            RuleMatchMode::All => {
                // Deny only if ALL rules denied
                // If any rule allowed, we already returned above
                // If we're here, check if all (non-skipped) rules denied
                (denied_count > 0 && denied_count == total_rules, denied_rule_name)
            }
        }
    }
}

#[async_trait]
impl RequestFilter for RequestRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Check for configuration errors
        if !self.config.is_valid() {
            let error = self.config.get_validation_error().unwrap_or("Unknown error");
            plugin_log.push(&format!("Config error: {}; ", error));
            // Allow request to proceed on config error (fail-open)
            return PluginRunningResult::GoodNext;
        }

        // Evaluate all rules
        let (denied, rule_name) = self.evaluate_rules(session);

        if denied {
            let rule_info = rule_name.unwrap_or_else(|| "unknown".to_string());
            plugin_log.push(&format!("Denied by {}; ", rule_info));

            let message = self
                .config
                .message
                .as_deref()
                .unwrap_or("Access denied by restriction policy");

            // Build error response
            let mut resp = Box::new(ResponseHeader::build(self.config.status, None).unwrap());
            resp.insert_header("Content-Type", "application/json").ok();

            let body = Bytes::from(format!(r#"{{"message":"{}"}}"#, message));

            // Write response
            if let Err(_e) = session.write_response_header(resp, false).await {
                return PluginRunningResult::ErrTerminateRequest;
            }

            if let Err(_e) = session.write_response_body(Some(body), true).await {
                return PluginRunningResult::ErrTerminateRequest;
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        plugin_log.push("Allowed; ");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::OnMissing;

    fn create_header_deny_config() -> RequestRestrictionConfig {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("block-bots".to_string()),
                source: RestrictionSource::Header,
                key: Some("User-Agent".to_string()),
                deny_regex: Some(vec!["(?i).*Bot.*".to_string(), "(?i).*Spider.*".to_string()]),
                on_missing: OnMissing::Allow,
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        config
    }

    fn create_path_allow_config() -> RequestRestrictionConfig {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("api-only".to_string()),
                source: RestrictionSource::Path,
                allow: Some(vec!["/health".to_string()]),
                allow_regex: Some(vec!["^/api/.*".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        config
    }

    fn create_method_allow_config() -> RequestRestrictionConfig {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("readonly".to_string()),
                source: RestrictionSource::Method,
                allow: Some(vec!["GET".to_string(), "HEAD".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        config
    }

    #[tokio::test]
    async fn test_header_deny_blocks_bot() {
        let config = create_header_deny_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session
            .expect_header_value()
            .withf(|k| k == "User-Agent")
            .return_const(Some("Googlebot/2.1".to_string()));
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }

    #[tokio::test]
    async fn test_header_deny_allows_normal_ua() {
        let config = create_header_deny_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session
            .expect_header_value()
            .withf(|k| k == "User-Agent")
            .return_const(Some("Mozilla/5.0 (Windows NT 10.0)".to_string()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Allowed"));
    }

    #[tokio::test]
    async fn test_path_allow_blocks_non_api() {
        let config = create_path_allow_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session.expect_get_path().return_const("/admin/users".to_string());
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }

    #[tokio::test]
    async fn test_path_allow_passes_api() {
        let config = create_path_allow_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session.expect_get_path().return_const("/api/users".to_string());

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Allowed"));
    }

    #[tokio::test]
    async fn test_path_allow_passes_health() {
        let config = create_path_allow_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session.expect_get_path().return_const("/health".to_string());

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Allowed"));
    }

    #[tokio::test]
    async fn test_method_allow_blocks_post() {
        let config = create_method_allow_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session.expect_get_method().return_const("POST".to_string());
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }

    #[tokio::test]
    async fn test_method_allow_passes_get() {
        let config = create_method_allow_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session.expect_get_method().return_const("GET".to_string());

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Allowed"));
    }

    #[tokio::test]
    async fn test_missing_header_allowed_by_default() {
        let config = create_header_deny_config();
        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session
            .expect_header_value()
            .withf(|k| k == "User-Agent")
            .return_const(None::<String>);

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Allowed"));
    }

    #[tokio::test]
    async fn test_missing_header_denied_when_configured() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("require-auth".to_string()),
                source: RestrictionSource::Header,
                key: Some("X-Auth-Token".to_string()),
                deny: Some(vec!["invalid".to_string()]),
                on_missing: OnMissing::Deny,
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();

        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session
            .expect_header_value()
            .withf(|k| k == "X-Auth-Token")
            .return_const(None::<String>);
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }

    #[tokio::test]
    async fn test_multiple_rules_any_mode() {
        // Any mode: first denial wins
        let mut config = RequestRestrictionConfig {
            rules: vec![
                RestrictionRule {
                    name: Some("block-bots".to_string()),
                    source: RestrictionSource::Header,
                    key: Some("User-Agent".to_string()),
                    deny_regex: Some(vec![".*Bot.*".to_string()]),
                    ..Default::default()
                },
                RestrictionRule {
                    name: Some("block-admin".to_string()),
                    source: RestrictionSource::Path,
                    deny_regex: Some(vec!["^/admin/.*".to_string()]),
                    ..Default::default()
                },
            ],
            match_mode: RuleMatchMode::Any,
            ..Default::default()
        };
        config.validate();

        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        // Normal UA but admin path - should be denied by path rule
        mock_session
            .expect_header_value()
            .withf(|k| k == "User-Agent")
            .return_const(Some("Mozilla/5.0".to_string()));
        mock_session.expect_get_path().return_const("/admin/users".to_string());
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }

    #[tokio::test]
    async fn test_cookie_restriction() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("block-debug".to_string()),
                source: RestrictionSource::Cookie,
                key: Some("debug".to_string()),
                deny: Some(vec!["true".to_string(), "1".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();

        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session
            .expect_get_cookie()
            .withf(|k| k == "debug")
            .return_const(Some("true".to_string()));
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }

    #[tokio::test]
    async fn test_query_restriction() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("block-test".to_string()),
                source: RestrictionSource::Query,
                key: Some("mode".to_string()),
                deny: Some(vec!["test".to_string(), "debug".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();

        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session
            .expect_get_query_param()
            .withf(|k| k == "mode")
            .return_const(Some("test".to_string()));
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }

    #[tokio::test]
    async fn test_referer_restriction() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("allow-internal".to_string()),
                source: RestrictionSource::Referer,
                allow_regex: Some(vec![".*example\\.com.*".to_string()]),
                on_missing: OnMissing::Deny,
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();

        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        mock_session
            .expect_header_value()
            .withf(|k| k == "Referer")
            .return_const(Some("https://evil.com/page".to_string()));
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Denied"));
    }
}

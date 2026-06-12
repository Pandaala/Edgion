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

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
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
        let _ = validated_config.validate();

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
            RestrictionSource::RequestPath => Some(session.get_request_path().to_string()),
            RestrictionSource::Method => Some(session.get_method().to_string()),
            RestrictionSource::Referer => session.header_value("Referer"),
        }
    }

    /// Evaluate all rules and determine if request should be denied
    /// Returns: (denied: bool, denied_source: Option<RestrictionSource>)
    fn evaluate_rules(&self, session: &dyn PluginSession) -> (bool, Option<RestrictionSource>) {
        let mut denied_count = 0;
        let mut denied_source = None;
        let total_rules = self.config.rules.len();

        for rule in &self.config.rules {
            let value = self.get_value(session, rule);
            let result = rule.check_value(value.as_deref());

            match result {
                Some(true) => {
                    // Rule triggered denial
                    denied_count += 1;
                    if denied_source.is_none() {
                        denied_source = Some(rule.source.clone());
                    }

                    // For "Any" mode, we can short-circuit on first denial
                    if self.config.match_mode == RuleMatchMode::Any {
                        return (true, denied_source);
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
                (denied_count > 0 && denied_count == total_rules, denied_source)
            }
        }
    }
}

#[async_trait]
impl RequestFilter for RequestRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        use edgion_resources::resources::edgion_plugins::PluginValidation;
        // Check for configuration errors
        if self.config.validation_error().is_some() {
            plugin_log.push_err("restriction config invalid");
            // invariant: 500 and 403 are compile-time-literal HTTP status codes; ResponseHeader::build cannot fail.
            let resp = Box::new(
                ResponseHeader::build(500, None)
                    .unwrap_or_else(|_| ResponseHeader::build(403, None).expect("403 is valid")),
            );
            let _ = session.write_response_header(resp, false).await;
            let _ = session
                .write_response_body(
                    Some(bytes::Bytes::from(r#"{"message":"Internal restriction policy error"}"#)),
                    true,
                )
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Evaluate all rules
        let (denied, denied_source) = self.evaluate_rules(session);

        if denied {
            let deny_msg = match denied_source {
                Some(RestrictionSource::Method) => "Deny method",
                Some(RestrictionSource::Path) => "Deny path",
                Some(RestrictionSource::RequestPath) => "Deny request path",
                Some(RestrictionSource::Header) => "Deny header",
                Some(RestrictionSource::Cookie) => "Deny cookie",
                Some(RestrictionSource::Query) => "Deny query",
                Some(RestrictionSource::Referer) => "Deny referer",
                None => "Deny rule",
            };
            plugin_log.push(deny_msg);

            let message = self
                .config
                .message
                .as_deref()
                .unwrap_or("Access denied by restriction policy");

            // Build error response.
            // invariant: 403 is a compile-time-literal HTTP status code; ResponseHeader::build cannot fail.
            let mut resp = Box::new(
                ResponseHeader::build(self.config.status, None)
                    .unwrap_or_else(|_| ResponseHeader::build(403, None).expect("403 is valid")),
            );
            resp.insert_header("Content-Type", "application/json").ok();

            let body = Bytes::from(serde_json::json!({"message": message}).to_string());

            // Write response
            if let Err(_e) = session.write_response_header(resp, false).await {
                plugin_log.push_err("write response header failed");
                return PluginRunningResult::ErrTerminateRequest;
            }

            if let Err(_e) = session.write_response_body(Some(body), true).await {
                plugin_log.push_err("write response body failed");
                return PluginRunningResult::ErrTerminateRequest;
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        plugin_log.push("Allow");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::OnMissing;

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
        let _ = config.validate();

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
        assert!(plugin_log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_request_path_source_uses_entry_raw() {
        // RequestPath source must call get_request_path(), not get_path().
        // The deny list value is kept raw (/%61dmin) and must match the raw entry path.
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                name: Some("deny-raw-path".to_string()),
                source: RestrictionSource::RequestPath,
                deny: Some(vec!["/%61dmin".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        let plugin = RequestRestriction::create(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("RequestRestriction");

        // Entry-time raw path is "/%61dmin" — should be denied
        mock_session
            .expect_get_request_path()
            .return_const("/%61dmin".to_string());
        mock_session.expect_write_response_header().returning(|_, _| Ok(()));
        mock_session.expect_write_response_body().returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;

        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(plugin_log.contains("Deny"));
    }
}

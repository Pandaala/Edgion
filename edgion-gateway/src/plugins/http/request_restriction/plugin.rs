//! Request Restriction plugin implementation
//!
//! This plugin restricts access based on request attributes like headers, cookies,
//! query parameters, path, method, and referer.
//!
//! ## Model (DNF: OR-of-AND)
//! - `ruleGroups` is OR: the policy fires if any group matches.
//! - Each group's `rules` is AND: a group matches only when all non-skipped
//!   predicates are true.
//! - A global `mode` decides the polarity: `Blacklist` (match => reject) or
//!   `Whitelist` (match => pass, otherwise reject).
//!
//! A rule is a pure predicate (`value ∈ values` OR `value` matches `regex`).
//! HashSet optimization kicks in for exact lists > 16; missing values are handled
//! per `onMissing` (`Match` / `NoMatch` / `Skip`).
//!
//! ## Configuration Examples:
//!
//! ### Blacklist: block bots OR /admin paths:
//! ```yaml
//! requestRestriction:
//!   mode: Blacklist
//!   ruleGroups:
//!     - rules:
//!         - source: Header
//!           key: "User-Agent"
//!           regex: ["(?i).*Bot.*", "(?i).*Spider.*"]
//!           onMissing: NoMatch
//!     - rules:
//!         - source: Path
//!           values: ["/admin"]
//!   message: "Bot access denied"
//! ```
//!
//! ### Whitelist: allow only specific paths:
//! ```yaml
//! requestRestriction:
//!   mode: Whitelist
//!   ruleGroups:
//!     - rules:
//!         - source: Path
//!           values: ["/health"]
//!           regex: ["^/api/.*"]
//! ```
//!
//! ### AND within a group: deny only DELETE to /admin:
//! ```yaml
//! requestRestriction:
//!   mode: Blacklist
//!   ruleGroups:
//!     - rules:
//!         - source: Method
//!           values: ["DELETE"]
//!         - source: Path
//!           regex: [".*/admin/.*"]
//! ```

use async_trait::async_trait;
use bytes::Bytes;

use crate::plugins::http::common::auth_common::send_gateway_plain_error_response;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{RequestRestrictionConfig, RestrictionMode, RuleGroup};

/// Request Restriction plugin
pub struct RequestRestriction {
    name: String,
    config: RequestRestrictionConfig,
    /// Validity computed once at construction. `Some` means the config is
    /// malformed (e.g. an invalid regex) and the plugin must fail closed.
    /// The config is immutable for the plugin's lifetime (a reload builds a new
    /// instance), so this is safe to cache and avoids re-validating per request.
    config_error: Option<String>,
}

impl RequestRestriction {
    /// Create a new RequestRestriction plugin from configuration
    pub fn create(config: &RequestRestrictionConfig) -> Box<dyn RequestFilter> {
        // Clone and validate the config to compile regex patterns; capture (not
        // discard) the validity so run_request can fail closed without recompiling.
        let mut validated_config = config.clone();
        let config_error = validated_config.validate().err();

        let plugin = RequestRestriction {
            name: "RequestRestriction".to_string(),
            config: validated_config,
            config_error,
        };

        Box::new(plugin)
    }

    /// A group matches when every non-skipped predicate is true and at least one
    /// predicate was considered (AND within the group).
    fn group_matches(&self, group: &RuleGroup, session: &dyn PluginSession) -> bool {
        let mut considered = 0usize;
        for rule in &group.rules {
            let values = crate::plugins::http::common::extract_rule_values(session, rule);
            match rule.evaluate_multi(&values) {
                None => continue, // Skip: drop from AND
                Some(true) => considered += 1,
                Some(false) => return false, // AND fails
            }
        }
        considered > 0
    }

    /// Evaluate all groups (OR across groups).
    /// Returns: (any_group_matched, matched_group_index).
    fn evaluate_groups(&self, session: &dyn PluginSession) -> (bool, Option<usize>) {
        for (i, group) in self.config.rule_groups.iter().enumerate() {
            if self.group_matches(group, session) {
                return (true, Some(i)); // short-circuit on first match
            }
        }
        (false, None)
    }
}

#[async_trait]
impl RequestFilter for RequestRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if self.config_error.is_some() {
            plugin_log.push_err("restriction config invalid");
            let body = Bytes::from(r#"{"message":"Internal restriction policy error"}"#);
            if let Err(_e) = send_gateway_plain_error_response(session, 500, "application/json", body).await {
                plugin_log.push_err("write response fail");
            }
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Evaluate all groups (OR), then apply the global mode polarity.
        let (any_matched, matched_idx) = self.evaluate_groups(session);
        let denied = match self.config.mode {
            RestrictionMode::Blacklist => any_matched,
            RestrictionMode::Whitelist => !any_matched,
        };

        if denied {
            let deny_msg = match (&self.config.mode, matched_idx) {
                (RestrictionMode::Blacklist, Some(i)) => {
                    let label = self.config.rule_groups[i]
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("group[{}]", i));
                    format!("Deny group {}", label)
                }
                (RestrictionMode::Whitelist, _) => "Deny (no allow group matched)".to_string(),
                _ => "Deny".to_string(),
            };
            plugin_log.push(&deny_msg);

            let message = self
                .config
                .message
                .as_deref()
                .unwrap_or("Access denied by restriction policy");

            // invariant: `check_fields` enforces 100 <= status < 600 before the plugin is
            // built, and a failed validation short-circuits above, so the helper's
            // `ResponseHeader::build` cannot fail on the configured status.
            let body = Bytes::from(serde_json::json!({"message": message}).to_string());

            if let Err(_e) =
                send_gateway_plain_error_response(session, self.config.status, "application/json", body).await
            {
                plugin_log.push_err("write response fail");
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
    use edgion_resources::resources::edgion_plugins::{
        OnMissing, RestrictionMode, RestrictionRule, RestrictionSource, RuleGroup,
    };

    fn cfg(mode: RestrictionMode, groups: Vec<RuleGroup>) -> RequestRestrictionConfig {
        let mut c = RequestRestrictionConfig {
            mode,
            rule_groups: groups,
            ..Default::default()
        };
        let _ = c.validate();
        c
    }

    #[tokio::test]
    async fn test_blacklist_missing_header_match_denies() {
        // onMissing: Match => absent X-Auth-Token counts as a hit => deny.
        let config = cfg(
            RestrictionMode::Blacklist,
            vec![RuleGroup {
                name: Some("missing-token".to_string()),
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X-Auth-Token".to_string()),
                    on_missing: OnMissing::Match,
                    ..Default::default()
                }],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_header_value()
            .withf(|k| k == "X-Auth-Token")
            .return_const(None::<String>);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_request_path_source_uses_entry_raw() {
        // RequestPath source must call get_request_path(), not get_path().
        // The value is kept raw (/%61dmin) and must match the raw entry path.
        let config = cfg(
            RestrictionMode::Blacklist,
            vec![RuleGroup {
                name: Some("deny-raw-path".to_string()),
                rules: vec![RestrictionRule {
                    source: RestrictionSource::RequestPath,
                    values: Some(vec!["/%61dmin".to_string()]),
                    ..Default::default()
                }],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_get_request_path().return_const("/%61dmin".to_string());
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_blacklist_and_group_needs_all_predicates() {
        // Group = (Method == DELETE) AND (Path == /admin). DELETE /other must pass.
        let config = cfg(
            RestrictionMode::Blacklist,
            vec![RuleGroup {
                name: Some("delete-admin".to_string()),
                rules: vec![
                    RestrictionRule {
                        source: RestrictionSource::Method,
                        values: Some(vec!["DELETE".to_string()]),
                        ..Default::default()
                    },
                    RestrictionRule {
                        source: RestrictionSource::Path,
                        values: Some(vec!["/admin".to_string()]),
                        ..Default::default()
                    },
                ],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_get_method().return_const("DELETE".to_string());
        s.expect_get_path().return_const("/other".to_string());

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("Allow"));
    }

    #[tokio::test]
    async fn test_whitelist_denies_when_no_group_matches() {
        // Whitelist: only GET passes. POST => deny.
        let config = cfg(
            RestrictionMode::Whitelist,
            vec![RuleGroup {
                name: Some("readonly".to_string()),
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Method,
                    values: Some(vec!["GET".to_string()]),
                    ..Default::default()
                }],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_get_method().return_const("POST".to_string());
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_skip_predicate_dropped_then_remaining_predicates_and() {
        // Group = (X-Opt Skip-on-missing) AND (Method DELETE).
        // X-Opt absent => that predicate is dropped; the group still ANDs the rest.
        let config = cfg(
            RestrictionMode::Blacklist,
            vec![RuleGroup {
                name: Some("skip-then-and".to_string()),
                rules: vec![
                    RestrictionRule {
                        source: RestrictionSource::Header,
                        key: Some("X-Opt".to_string()),
                        values: Some(vec!["v".to_string()]),
                        on_missing: OnMissing::Skip,
                        ..Default::default()
                    },
                    RestrictionRule {
                        source: RestrictionSource::Method,
                        values: Some(vec!["DELETE".to_string()]),
                        ..Default::default()
                    },
                ],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_header_value()
            .withf(|k| k == "X-Opt")
            .return_const(None::<String>);
        s.expect_get_method().return_const("DELETE".to_string());
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        // Skip drops X-Opt; Method matches => considered=1, all true => group matches => deny.
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_all_skip_group_blacklist_passes() {
        // A group whose only predicate is Skip-on-missing yields considered==0 =>
        // group does not match => Blacklist => not denied (fail-open).
        let config = cfg(
            RestrictionMode::Blacklist,
            vec![RuleGroup {
                name: Some("all-skip".to_string()),
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X-Opt".to_string()),
                    values: Some(vec!["v".to_string()]),
                    on_missing: OnMissing::Skip,
                    ..Default::default()
                }],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_header_value()
            .withf(|k| k == "X-Opt")
            .return_const(None::<String>);

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("Allow"));
    }

    #[tokio::test]
    async fn test_all_skip_group_whitelist_denies() {
        // Same all-Skip group under Whitelist: considered==0 => no group matched =>
        // Whitelist => deny (fail-closed). Documents the foot-gun.
        let config = cfg(
            RestrictionMode::Whitelist,
            vec![RuleGroup {
                name: Some("all-skip".to_string()),
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X-Opt".to_string()),
                    values: Some(vec!["v".to_string()]),
                    on_missing: OnMissing::Skip,
                    ..Default::default()
                }],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_header_value()
            .withf(|k| k == "X-Opt")
            .return_const(None::<String>);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_whitelist_multi_group_or_passes_on_second_group() {
        // Whitelist OR across groups: (Method GET) OR (X-Tier gold). A POST with
        // X-Tier: gold matches the second group => pass.
        let config = cfg(
            RestrictionMode::Whitelist,
            vec![
                RuleGroup {
                    name: Some("readonly".to_string()),
                    rules: vec![RestrictionRule {
                        source: RestrictionSource::Method,
                        values: Some(vec!["GET".to_string()]),
                        ..Default::default()
                    }],
                },
                RuleGroup {
                    name: Some("gold-tier".to_string()),
                    rules: vec![RestrictionRule {
                        source: RestrictionSource::Header,
                        key: Some("X-Tier".to_string()),
                        values: Some(vec!["gold".to_string()]),
                        ..Default::default()
                    }],
                },
            ],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_get_method().return_const("POST".to_string());
        s.expect_header_value()
            .withf(|k| k == "X-Tier")
            .return_const(Some("gold".to_string()));

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("Allow"));
    }

    #[tokio::test]
    async fn test_invalid_config_fails_closed() {
        // An invalid regex fails validation at construction; run_request must fail
        // closed (terminate) rather than evaluate a half-compiled config.
        let config = cfg(
            RestrictionMode::Blacklist,
            vec![RuleGroup {
                name: Some("bad".to_string()),
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X".to_string()),
                    regex: Some(vec!["[invalid".to_string()]),
                    ..Default::default()
                }],
            }],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }
}

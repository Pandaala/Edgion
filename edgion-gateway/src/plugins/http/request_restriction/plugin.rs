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
use crate::plugins::http::common::identity_source::ExtractedValues;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{RequestRestrictionConfig, RestrictionMode, RuleGroup};
use edgion_resources::EdgionStatus;

/// Outcome of evaluating one rule group's AND (Step 4 of Task 10).
enum GroupOutcome {
    Matched,
    NotMatched,
    /// A geo rule in this group declared `failOpen: false` and the GeoIP
    /// database is unavailable. Propagates out of `evaluate_groups` to reject
    /// the whole plugin, before the `mode` polarity is even applied — see the
    /// module doc on why a false predicate cannot express fail-closed.
    FailClosed,
}

/// Request Restriction plugin
pub struct RequestRestriction {
    name: String,
    config: RequestRestrictionConfig,
    /// Validity computed once at construction. `Some` means the config is
    /// malformed (e.g. an invalid regex) and the plugin must fail closed.
    /// The config is immutable for the plugin's lifetime (a reload builds a new
    /// instance), so this is safe to cache and avoids re-validating per request.
    config_error: Option<String>,
    /// `true` when `config.fail_open == Some(false)` — the plugin declared an
    /// explicit fail-closed disposition for its geo rules (only possible when
    /// the plugin has at least one). Computed once at construction (the
    /// config is immutable for the plugin's lifetime) so `run_request` can
    /// short-circuit to rejection at the very top — see the module doc and
    /// BLOCKER 2 of the 2026-08-15 GeoIP review: walking the DNF tree for
    /// this is order dependent (a `NotMatched` predicate earlier in the same
    /// AND group, or an earlier group that already matched, both stop the
    /// walk before a fail-closed geo rule is ever reached), so this must be a
    /// plugin-level property, not something discovered mid-walk. Now that
    /// `failOpen` itself lives on `RequestRestrictionConfig` (moved off
    /// `RestrictionRule` — see that struct's field doc), this is a direct
    /// field read rather than a scan over every rule in every group.
    has_fail_closed_geo_rule: bool,
    /// The plugin's resolved `failOpen` value, threaded into
    /// `extract_rule_values` for every geo rule. `false` when the plugin has
    /// no geo rule at all (never read in that case, since no rule can
    /// produce `ExtractedValues::ForceSkip`/`FailClosed`).
    geo_fail_open: bool,
}

impl RequestRestriction {
    /// Create a new RequestRestriction plugin from configuration
    pub fn create(config: &RequestRestrictionConfig) -> Box<dyn RequestFilter> {
        // Clone and validate the config to compile regex patterns; capture (not
        // discard) the validity so run_request can fail closed without recompiling.
        let mut validated_config = config.clone();
        let config_error = validated_config.validate().err();
        let geo_fail_open = validated_config.fail_open.unwrap_or(false);
        let has_fail_closed_geo_rule = validated_config.fail_open == Some(false);

        let plugin = RequestRestriction {
            name: "RequestRestriction".to_string(),
            config: validated_config,
            config_error,
            has_fail_closed_geo_rule,
            geo_fail_open,
        };

        Box::new(plugin)
    }

    /// Build the fail-closed rejection response shared by the plugin-level
    /// short circuit and the (now defence-in-depth) per-group `FailClosed`
    /// path below. Both must produce identical status/body/log/EdgionStatus.
    async fn respond_geo_fail_closed(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        session.add_error(EdgionStatus::GeoRestrictionFailClosed);
        plugin_log.push_err("geo fail-closed: GeoIP database unavailable");

        let message = self
            .config
            .message
            .as_deref()
            .unwrap_or("Access denied by restriction policy");
        let body = Bytes::from(serde_json::json!({"message": message}).to_string());
        if let Err(_e) = send_gateway_plain_error_response(session, self.config.status, "application/json", body).await
        {
            plugin_log.push_err("write response fail");
        }
    }

    /// A group matches when every non-skipped predicate is true and at least one
    /// predicate was considered (AND within the group). A geo rule that finds the
    /// database unavailable either drops out (`ForceSkip`, `failOpen: true`) or
    /// aborts the whole group evaluation (`FailClosed`, `failOpen: false`).
    fn group_matches(&self, group: &RuleGroup, session: &dyn PluginSession) -> GroupOutcome {
        let mut considered = 0usize;
        for rule in &group.rules {
            match crate::plugins::http::common::extract_rule_values(session, rule, self.geo_fail_open) {
                ExtractedValues::FailClosed => return GroupOutcome::FailClosed,
                ExtractedValues::ForceSkip => continue,
                ExtractedValues::Values(values) => match rule.evaluate_multi(&values) {
                    None => continue, // Skip: drop from AND
                    Some(true) => considered += 1,
                    Some(false) => return GroupOutcome::NotMatched, // AND fails
                },
            }
        }
        if considered > 0 {
            GroupOutcome::Matched
        } else {
            GroupOutcome::NotMatched
        }
    }

    /// Evaluate all groups (OR across groups).
    ///
    /// Returns `Err(())` the instant any group reports `FailClosed` — that
    /// short-circuits the whole plugin to rejection regardless of `mode` and
    /// regardless of any other group, so scanning stops immediately rather than
    /// continuing to look for an ordinary match.
    fn evaluate_groups(&self, session: &dyn PluginSession) -> Result<(bool, Option<usize>), ()> {
        for (i, group) in self.config.rule_groups.iter().enumerate() {
            match self.group_matches(group, session) {
                GroupOutcome::Matched => return Ok((true, Some(i))), // short-circuit on first match
                GroupOutcome::NotMatched => continue,
                GroupOutcome::FailClosed => return Err(()),
            }
        }
        Ok((false, None))
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

        // Plugin-level short circuit (BLOCKER 2 of the 2026-08-15 review): a
        // `failOpen: false` rule must reject "regardless of mode and
        // regardless of any other rule group" (design spec §5.2). Checking
        // this via the DNF walk below is order-dependent — a `NotMatched`
        // predicate earlier in the same AND group, or an earlier OR group that
        // already matched, both stop the walk before a later fail-closed geo
        // rule is ever reached. Hoisting it here, before any group is
        // evaluated, makes the short circuit unconditional and — as a
        // side-effect — cheaper: one `ArcSwap` load instead of walking groups.
        if self.has_fail_closed_geo_rule && !crate::geoip::GeoIpProvider::global().is_loaded() {
            self.respond_geo_fail_closed(session, plugin_log).await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Evaluate all groups (OR), then apply the global mode polarity.
        //
        // `GroupOutcome::FailClosed` / `ExtractedValues::FailClosed` are kept
        // as defence-in-depth rather than deleted: `GeoLookup::Unavailable` is
        // produced exactly when `GeoIpProvider::global().is_loaded()` is
        // false (see `ctx.rs::resolve_geo` -> `GeoIpProvider::lookup`), so the
        // plugin-level check above is a sound, provably-equivalent
        // precondition for every path that can reach this branch today — but
        // keeping the per-rule fallback costs nothing and protects against a
        // future desync between the two (e.g. a new `GeoLookup::Unavailable`
        // producer that the plugin-level flag doesn't account for).
        let Ok((any_matched, matched_idx)) = self.evaluate_groups(session) else {
            self.respond_geo_fail_closed(session, plugin_log).await;
            return PluginRunningResult::ErrTerminateRequest;
        };
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

    mod geo {
        use super::*;
        use crate::geoip::{GeoIpProvider, GeoLookup, GEO_TEST_LOCK};

        fn geo_rule(key: &str, values: &[&str]) -> RestrictionRule {
            RestrictionRule {
                source: RestrictionSource::ClientGeo,
                key: Some(key.to_string()),
                values: Some(values.iter().map(|s| s.to_string()).collect()),
                ..Default::default()
            }
        }

        /// Build the plugin against the given config with the process-global
        /// GeoIP provider forced unloaded, run one request, and report whether
        /// the plugin denied it.
        async fn evaluate_denies(cfg: &RequestRestrictionConfig) -> bool {
            {
                let _guard = GEO_TEST_LOCK.lock();
                GeoIpProvider::global().clear();
            }

            let plugin = RequestRestriction::create(cfg);
            let mut s = MockPluginSession::new();
            s.expect_client_geo().returning(|| GeoLookup::Unavailable);
            s.expect_write_response_header().returning(|_, _| Ok(()));
            s.expect_write_response_body().returning(|_, _| Ok(()));
            s.expect_shutdown().returning(|| {});
            s.expect_add_error().returning(|_| {});
            let mut log = PluginLog::new("RequestRestriction");
            plugin.run_request(&mut s, &mut log).await == PluginRunningResult::ErrTerminateRequest
        }

        /// Same as `evaluate_denies`, but lets the caller stage additional
        /// mock expectations (e.g. a non-geo predicate) before the request
        /// runs — needed to build the "geo rule not first / not in the first
        /// group" scenarios the plugin-level short circuit must still cover.
        async fn evaluate_denies_with(
            cfg: &RequestRestrictionConfig,
            stage: impl FnOnce(&mut MockPluginSession),
        ) -> bool {
            {
                let _guard = GEO_TEST_LOCK.lock();
                GeoIpProvider::global().clear();
            }

            let plugin = RequestRestriction::create(cfg);
            let mut s = MockPluginSession::new();
            s.expect_client_geo().returning(|| GeoLookup::Unavailable);
            s.expect_write_response_header().returning(|_, _| Ok(()));
            s.expect_write_response_body().returning(|_, _| Ok(()));
            s.expect_shutdown().returning(|| {});
            s.expect_add_error().returning(|_| {});
            stage(&mut s);
            let mut log = PluginLog::new("RequestRestriction");
            plugin.run_request(&mut s, &mut log).await == PluginRunningResult::ErrTerminateRequest
        }

        /// failOpen: false + no database => reject, in BLACKLIST mode. A
        /// false predicate would mean "allow" here, which is the whole reason
        /// on_missing cannot express fail-closed.
        #[tokio::test]
        async fn fail_closed_rejects_in_blacklist_mode_when_db_unavailable() {
            let cfg = RequestRestrictionConfig {
                mode: RestrictionMode::Blacklist,
                rule_groups: vec![RuleGroup {
                    name: Some("geo".into()),
                    rules: vec![geo_rule("country", &["RU"])],
                }],
                status: 403,
                message: None,
                fail_open: Some(false),
            };
            // Provider unloaded in this test process.
            assert!(evaluate_denies(&cfg).await);
        }

        /// Same config, whitelist mode: still rejects.
        #[tokio::test]
        async fn fail_closed_rejects_in_whitelist_mode_when_db_unavailable() {
            let cfg = RequestRestrictionConfig {
                mode: RestrictionMode::Whitelist,
                rule_groups: vec![RuleGroup {
                    name: Some("geo".into()),
                    rules: vec![geo_rule("country", &["CN"])],
                }],
                status: 403,
                message: None,
                fail_open: Some(false),
            };
            assert!(evaluate_denies(&cfg).await);
        }

        /// failOpen: true => the predicate is dropped from the AND, exactly
        /// like Skip, regardless of the rule's own on_missing.
        #[tokio::test]
        async fn fail_open_drops_the_predicate_when_db_unavailable() {
            let mut rule = geo_rule("country", &["RU"]);
            rule.on_missing = OnMissing::NoMatch; // must be overridden by fail-open
            let cfg = RequestRestrictionConfig {
                mode: RestrictionMode::Blacklist,
                rule_groups: vec![RuleGroup {
                    name: Some("geo".into()),
                    rules: vec![rule],
                }],
                status: 403,
                message: None,
                fail_open: Some(true),
            };
            assert!(!evaluate_denies(&cfg).await);
        }

        /// BLOCKER 2 regression: `failOpen: false` must reject regardless of
        /// *any other rule group*, even one that already matched. Group 1
        /// matches on a header (an ordinary Blacklist rule); group 2 holds the
        /// fail-closed geo rule. Before the plugin-level short circuit, the
        /// DNF walk in `evaluate_groups` returned `Matched` on group 1 and
        /// never reached group 2's `FailClosed` at all.
        #[tokio::test]
        async fn fail_closed_geo_rule_in_a_later_group_still_rejects_in_blacklist_mode() {
            let cfg = RequestRestrictionConfig {
                mode: RestrictionMode::Blacklist,
                rule_groups: vec![
                    RuleGroup {
                        name: Some("header-match".into()),
                        rules: vec![RestrictionRule {
                            source: RestrictionSource::Header,
                            key: Some("X-Scanner".to_string()),
                            values: Some(vec!["1".to_string()]),
                            ..Default::default()
                        }],
                    },
                    RuleGroup {
                        name: Some("geo".into()),
                        rules: vec![geo_rule("country", &["RU"])],
                    },
                ],
                status: 403,
                message: None,
                fail_open: Some(false),
            };
            let denies = evaluate_denies_with(&cfg, |s| {
                s.expect_header_value()
                    .withf(|k| k == "X-Scanner")
                    .return_const(Some("1".to_string()));
            })
            .await;
            assert!(
                denies,
                "failOpen: false must reject even though an earlier group already matched"
            );
        }

        /// Same escape, Whitelist mode: an earlier group matching (which would
        /// normally ALLOW under Whitelist) must not let the request through
        /// past a later fail-closed geo rule.
        #[tokio::test]
        async fn fail_closed_geo_rule_in_a_later_group_still_rejects_in_whitelist_mode() {
            let cfg = RequestRestrictionConfig {
                mode: RestrictionMode::Whitelist,
                rule_groups: vec![
                    RuleGroup {
                        name: Some("header-match".into()),
                        rules: vec![RestrictionRule {
                            source: RestrictionSource::Header,
                            key: Some("X-Scanner".to_string()),
                            values: Some(vec!["1".to_string()]),
                            ..Default::default()
                        }],
                    },
                    RuleGroup {
                        name: Some("geo".into()),
                        rules: vec![geo_rule("country", &["RU"])],
                    },
                ],
                status: 403,
                message: None,
                fail_open: Some(false),
            };
            let denies = evaluate_denies_with(&cfg, |s| {
                s.expect_header_value()
                    .withf(|k| k == "X-Scanner")
                    .return_const(Some("1".to_string()));
            })
            .await;
            assert!(
                denies,
                "failOpen: false must reject regardless of an earlier matching Whitelist group"
            );
        }

        /// BLOCKER 2 regression: the geo rule is not the first predicate in its
        /// own AND group. Before the plugin-level short circuit,
        /// `group_matches` returned `NotMatched` on the earlier false header
        /// predicate before ever reaching the fail-closed geo rule.
        #[tokio::test]
        async fn fail_closed_geo_rule_not_first_in_its_and_group_still_rejects_in_blacklist_mode() {
            let cfg = RequestRestrictionConfig {
                mode: RestrictionMode::Blacklist,
                rule_groups: vec![RuleGroup {
                    name: Some("and-group".into()),
                    rules: vec![
                        RestrictionRule {
                            source: RestrictionSource::Header,
                            key: Some("X-Present".to_string()),
                            values: Some(vec!["only-this-value".to_string()]),
                            ..Default::default()
                        },
                        geo_rule("country", &["RU"]),
                    ],
                }],
                status: 403,
                message: None,
                fail_open: Some(false),
            };
            let denies = evaluate_denies_with(&cfg, |s| {
                // Header present but does NOT match the required value, so the
                // first predicate in the AND group is false.
                s.expect_header_value()
                    .withf(|k| k == "X-Present")
                    .return_const(Some("something-else".to_string()));
            })
            .await;
            assert!(
                denies,
                "failOpen: false must reject even when an earlier AND predicate in its own group is false"
            );
        }

        /// Same escape, Whitelist mode.
        #[tokio::test]
        async fn fail_closed_geo_rule_not_first_in_its_and_group_still_rejects_in_whitelist_mode() {
            let cfg = RequestRestrictionConfig {
                mode: RestrictionMode::Whitelist,
                rule_groups: vec![RuleGroup {
                    name: Some("and-group".into()),
                    rules: vec![
                        RestrictionRule {
                            source: RestrictionSource::Header,
                            key: Some("X-Present".to_string()),
                            values: Some(vec!["only-this-value".to_string()]),
                            ..Default::default()
                        },
                        geo_rule("country", &["RU"]),
                    ],
                }],
                status: 403,
                message: None,
                fail_open: Some(false),
            };
            let denies = evaluate_denies_with(&cfg, |s| {
                s.expect_header_value()
                    .withf(|k| k == "X-Present")
                    .return_const(Some("something-else".to_string()));
            })
            .await;
            assert!(
                denies,
                "failOpen: false must reject even when an earlier AND predicate in its own group is false"
            );
        }

        /// There is no "database tier" concept in the data model (final review,
        /// 2026-08-15: the earlier `check_database_tier` heuristic was removed
        /// — it string-matched `database_type` for `"City"`, which both
        /// false-warned on `GeoIP2-Enterprise` and silently never fired on a
        /// cold start with no database loaded, since `database_type()` is
        /// `None` at plugin construction in the normal deployment where no
        /// database ships in the image). A parsed record is uniform regardless
        /// of product/version: `city` is `None` whether the database carries
        /// no city data at all, this record specifically lacks one, or the
        /// address didn't resolve — all three are ordinary "value absent",
        /// governed by `onMissing` exactly like any other source. This test
        /// proves that real contract: a `city` predicate against a record with
        /// no city data is `on_missing`-governed, not a distinct failure mode.
        /// `fail_open: true` (not `false`) is deliberate here, not incidental:
        /// these two tests are about the `onMissing`-governed "value absent"
        /// path, which is orthogonal to BLOCKER 2's `failOpen: false`
        /// plugin-level short circuit (covered by its own tests above). Using
        /// `fail_open: true` means `has_fail_closed_geo_rule` is `false`, so
        /// the short circuit never engages and these tests need not depend on
        /// `GeoIpProvider::global()`'s real load state at all — avoiding a
        /// cross-test race on that process-global singleton (this test file
        /// runs many geo tests in parallel threads that load/clear it).
        fn config_with_city_rule(on_missing: OnMissing) -> RequestRestrictionConfig {
            let mut rule = geo_rule("city", &["Beijing"]);
            rule.on_missing = on_missing;
            let mut c = RequestRestrictionConfig {
                mode: RestrictionMode::Blacklist,
                rule_groups: vec![RuleGroup {
                    name: Some("geo-city".into()),
                    rules: vec![rule],
                }],
                fail_open: Some(true),
                ..Default::default()
            };
            let _ = c.validate();
            c
        }

        /// A `Found` lookup whose City/Enterprise-only dimensions are absent —
        /// exactly what a Country-tier database (or any build lacking city
        /// data) produces for every address, real fixture or not.
        fn country_only_geo_info() -> crate::geoip::GeoInfo {
            crate::geoip::GeoInfo {
                country_iso: Some("CN".to_string()),
                continent: Some("AS".to_string()),
                subdivision: None,
                city: None,
                latitude: None,
                longitude: None,
            }
        }

        #[tokio::test]
        async fn missing_city_dimension_matches_when_on_missing_is_match() {
            let cfg = config_with_city_rule(OnMissing::Match);
            let plugin = RequestRestriction::create(&cfg);
            let mut s = MockPluginSession::new();
            s.expect_client_geo()
                .returning(|| GeoLookup::Found(std::sync::Arc::new(country_only_geo_info())));
            s.expect_write_response_header().returning(|_, _| Ok(()));
            s.expect_write_response_body().returning(|_, _| Ok(()));
            s.expect_shutdown().returning(|| {});
            let mut log = PluginLog::new("RequestRestriction");
            let result = plugin.run_request(&mut s, &mut log).await;
            assert_eq!(
                result,
                PluginRunningResult::ErrTerminateRequest,
                "onMissing: Match must deny when the city dimension is absent"
            );
        }

        #[tokio::test]
        async fn missing_city_dimension_does_not_match_when_on_missing_is_no_match() {
            let cfg = config_with_city_rule(OnMissing::NoMatch);
            let plugin = RequestRestriction::create(&cfg);
            let mut s = MockPluginSession::new();
            s.expect_client_geo()
                .returning(|| GeoLookup::Found(std::sync::Arc::new(country_only_geo_info())));
            let mut log = PluginLog::new("RequestRestriction");
            let result = plugin.run_request(&mut s, &mut log).await;
            assert_eq!(
                result,
                PluginRunningResult::GoodNext,
                "onMissing: NoMatch must allow when the city dimension is absent"
            );
        }
    }
}

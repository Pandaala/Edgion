//! GlobalAccessControl runtime plugin (L7 request stage).
//!
//! Resolves the active profile via the Selector EdgionConfigData store (fail-to-base),
//! then evaluates the Plan-1 allow engine against the live request. Unlike IpRestriction,
//! this plugin FAILS OPEN on init/compile error or an unresolvable profile: it allows the
//! request and records the event (access log + tracing). This matches the operator
//! decision that a global allow-list must not take down all traffic on a bad config.

use async_trait::async_trait;
use bytes::Bytes;
use std::net::IpAddr;

use crate::plugins::http::common::auth_common::send_gateway_plain_error_response;
use crate::plugins::http::common::identity_source::ExtractedValues;
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::matcher::ip_cidr_helpers::evaluate_ip_access;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    AllowOutcome, AllowProfile, AllowRuleGroup, GlobalAccessControlConfig,
};
use edgion_resources::resources::edgion_plugins::{AccessDecision, DefaultAction, IpSource, RestrictionSource};
use edgion_resources::EdgionStatus;

/// Outcome of evaluating one `AllowRuleGroup` under geo tri-state semantics.
/// Mirrors `request_restriction`'s `GroupOutcome`, reimplemented here because
/// `AllowRuleGroup::matches` (in `edgion-resources`) is closure-based over
/// `Vec<String>` and cannot express the fail-closed signal — `ExtractedValues`
/// is gateway-only (it carries `GeoLookup` semantics), so this evaluation
/// cannot be pushed down into the shared resources crate.
enum GacGroupOutcome {
    Matched(Option<String>),
    NotMatched,
    /// A geo rule in this group declared `failOpen: false` and the GeoIP
    /// database is unavailable. GAC's usual fail-open-on-init-error posture
    /// does NOT apply here: a `failOpen: false` rule is what the operator
    /// asked for, and it must still reject.
    FailClosed,
}

/// Evaluate one allow group: IP/CIDR precondition (if any) AND every
/// non-skipped rule, with the same tri-state geo handling as
/// `request_restriction::RequestRestriction::group_matches`.
///
/// `geo_fail_open` is the plugin's resolved `failOpen` value (see
/// `GlobalAccessControlConfig::fail_open`), threaded down to
/// `extract_rule_values` for every geo rule in `group`.
fn group_outcome(
    group: &AllowRuleGroup,
    ip: Option<&IpAddr>,
    session: &dyn PluginSession,
    geo_fail_open: bool,
) -> GacGroupOutcome {
    let mut considered = false;
    if let Some(matcher) = &group.ip_matcher {
        // IP constraint present but client IP unknown => cannot verify => fail closed.
        let Some(ip) = ip else {
            return GacGroupOutcome::NotMatched;
        };
        let in_set = matches!(
            evaluate_ip_access(None, Some(matcher), &DefaultAction::Deny, ip),
            AccessDecision::AllowByGroup(_)
        );
        if !in_set {
            return GacGroupOutcome::NotMatched;
        }
        considered = true;
    }
    for rule in &group.rules {
        match crate::plugins::http::common::extract_rule_values(session, rule, geo_fail_open) {
            ExtractedValues::FailClosed => return GacGroupOutcome::FailClosed,
            ExtractedValues::ForceSkip => continue,
            ExtractedValues::Values(values) => match rule.evaluate_multi(&values) {
                None => continue, // Skip: drop from AND
                Some(true) => considered = true,
                Some(false) => return GacGroupOutcome::NotMatched,
            },
        }
    }
    if considered {
        GacGroupOutcome::Matched(group.name.clone())
    } else {
        GacGroupOutcome::NotMatched
    }
}

/// Evaluate a profile with tri-state geo handling. `Err(())` means some group
/// reported `FailClosed`; the caller rejects immediately rather than falling
/// through to `default_action`.
fn evaluate_profile(
    profile: &AllowProfile,
    ip: Option<&IpAddr>,
    session: &dyn PluginSession,
    geo_fail_open: bool,
) -> Result<AllowOutcome, ()> {
    for group in &profile.rule_groups {
        match group_outcome(group, ip, session, geo_fail_open) {
            GacGroupOutcome::Matched(name) => return Ok(AllowOutcome::Allowed { group: name }),
            GacGroupOutcome::NotMatched => continue,
            GacGroupOutcome::FailClosed => return Err(()),
        }
    }
    Ok(match profile.default_action {
        DefaultAction::Allow => AllowOutcome::Allowed { group: None },
        DefaultAction::Deny => AllowOutcome::Denied,
    })
}

/// Warn at construction when a group's IP precondition and its geo rule
/// disagree about which address they decide from.
///
/// GAC's `IpSource::ClientIp` maps to `session.remote_addr()` (real client,
/// post-XFF) and `IpSource::RemoteAddr` maps to `session.client_addr()` (TCP
/// peer) — the inverse of the `RequestInfo` naming that `RestrictionSource::
/// ClientGeo` / `DirectGeo` follow (`ClientGeo` reads `remote_addr`, `DirectGeo`
/// reads `client_addr`). So `ip_source: ClientIp` (remote_addr) paired with a
/// `DirectGeo` rule (client_addr) — or the reverse — makes one AND group
/// decide from two different addresses without any error ever surfacing.
fn ip_source_geo_disagreements(config: &GlobalAccessControlConfig) -> Vec<String> {
    let mismatched_source = match config.ip_source {
        IpSource::ClientIp => RestrictionSource::DirectGeo,
        IpSource::RemoteAddr => RestrictionSource::ClientGeo,
    };
    let mut warnings = Vec::new();
    for (pname, profile) in &config.profiles {
        for (gi, group) in profile.rule_groups.iter().enumerate() {
            let has_ip_precondition = group.ip.as_ref().is_some_and(|g| !g.is_empty());
            if !has_ip_precondition {
                continue;
            }
            for rule in &group.rules {
                if rule.source == mismatched_source {
                    let label = group.name.clone().unwrap_or_else(|| format!("group[{}]", gi));
                    warnings.push(format!(
                        "profile '{}' group '{}': ipSource {:?} disagrees with rule source {:?} — the IP precondition and the geo predicate decide from different addresses",
                        pname, label, config.ip_source, rule.source
                    ));
                }
            }
        }
    }
    warnings
}

/// GlobalAccessControl L7 allow-list plugin.
pub struct GlobalAccessControl {
    name: String,
    config: GlobalAccessControlConfig,
    /// True when `validate_and_init` failed at construction. Requests then FAIL OPEN
    /// (allowed) rather than be evaluated against uncompiled matchers.
    init_failed: bool,
    /// Resolved "namespace/name" key for the activeProfileRef Selector (built once;
    /// re-looked-up live each request for hot reload). None when no selector configured.
    active_profile_ref_key: Option<String>,
    /// `true` when `config.fail_open == Some(false)`. Computed once at
    /// construction so `run_request` can short-circuit to rejection before
    /// walking any group — see BLOCKER 2 of the 2026-08-15 GeoIP review and
    /// the identical field on `request_restriction::RequestRestriction`.
    /// Plugin-wide rather than per-profile: `failOpen` moved off
    /// `RestrictionRule` onto `GlobalAccessControlConfig` (see that field's
    /// doc), so it governs whichever profile the Selector resolves at
    /// request time, not just the profile it was declared under.
    has_fail_closed_geo_rule: bool,
    /// The plugin's resolved `failOpen` value, threaded into
    /// `extract_rule_values` for every geo rule. `false` when the plugin has
    /// no geo rule at all (never read in that case).
    geo_fail_open: bool,
}

impl GlobalAccessControl {
    pub fn create(config: &GlobalAccessControlConfig, namespace: &str) -> Box<dyn RequestFilter> {
        // Matchers (ip_matcher, rule values_set/compiled_regex) are `#[serde(skip)]`, so
        // they are None after gRPC/YAML deserialization; compile them here.
        let mut config = config.clone();
        let init_failed = if let Err(e) = config.validate_and_init() {
            tracing::error!(
                error = %e,
                label = "HTTP plugin",
                "Failed to initialize GlobalAccessControlConfig; failing OPEN (allowing all requests)",
            );
            true
        } else {
            false
        };

        let active_profile_ref_key = config
            .active_profile_ref
            .as_ref()
            .filter(|r| r.ref_denied.is_none())
            .map(|r| {
                let ns = r.namespace.as_deref().unwrap_or(namespace);
                format!("{}/{}", ns, r.name)
            });

        for warning in ip_source_geo_disagreements(&config) {
            tracing::warn!(label = "HTTP plugin", warning = %warning, "GlobalAccessControl ipSource/geo disagreement");
        }

        let geo_fail_open = config.fail_open.unwrap_or(false);
        let has_fail_closed_geo_rule = config.fail_open == Some(false);

        Box::new(GlobalAccessControl {
            name: "GlobalAccessControl".to_string(),
            config,
            init_failed,
            active_profile_ref_key,
            has_fail_closed_geo_rule,
            geo_fail_open,
        })
    }

    /// Resolve the active profile name via the Selector store, with fail-to-base to
    /// `config.active_profile` on any error/absence. Mirrors the L4 GCIR pattern.
    ///
    /// When a selector IS configured but cannot be honored (EdgionConfigData missing,
    /// kill-switched, wrong variant, or naming an unknown profile), a `selector miss: base`
    /// line is recorded so operators can see the fallback in the access log. A selector
    /// with `active: null` is a deliberate no-op (use base) and is NOT a miss.
    fn resolve_active_profile_name(&self, plugin_log: &mut PluginLog) -> String {
        let Some(ref key) = self.active_profile_ref_key else {
            return self.config.active_profile.clone();
        };
        let store = get_global_edgion_config_data_store();
        let raw_ed = store.get_raw_by_key(key);

        // Envelope kill-switch: spec.enable=false => ignore the selector, use base.
        if raw_ed.as_ref().map(|ed| !ed.spec.enable).unwrap_or(false) {
            plugin_log.push("selector miss: base");
            return self.config.active_profile.clone();
        }
        match raw_ed.as_deref().map(|ed| &ed.spec.data) {
            None => {
                plugin_log.push("selector miss: base");
                self.config.active_profile.clone()
            }
            Some(ConfigEntry::Selector(sel)) => match sel.active.as_deref() {
                // Deliberate no-op (use base) — not a miss.
                None => self.config.active_profile.clone(),
                Some(name) if self.config.profiles.contains_key(name) => name.to_string(),
                Some(_) => {
                    plugin_log.push("selector miss: base");
                    self.config.active_profile.clone()
                }
            },
            Some(_) => {
                plugin_log.push("selector miss: base");
                self.config.active_profile.clone()
            }
        }
    }

    /// Extract the client IP per the configured source. RemoteAddr = TCP peer (un-forgeable).
    fn get_client_ip(&self, session: &dyn PluginSession) -> Option<IpAddr> {
        let ip_str = match self.config.ip_source {
            IpSource::ClientIp => session.remote_addr(),
            IpSource::RemoteAddr => session.client_addr(),
        };
        if ip_str.is_empty() {
            return None;
        }
        ip_str.parse::<IpAddr>().ok()
    }

    /// Shared fail-closed rejection response — used by both the plugin-level
    /// short circuit and the (now defence-in-depth) per-group `FailClosed`
    /// path in `run_request`.
    async fn respond_geo_fail_closed(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        session.add_error(EdgionStatus::GeoRestrictionFailClosed);
        plugin_log.push_err("geo fail-closed: GeoIP database unavailable");
        let message = self.config.message.as_deref().unwrap_or("Access denied");
        let body = Bytes::from(serde_json::json!({ "message": message }).to_string());
        if let Err(_e) = send_gateway_plain_error_response(session, self.config.status, "application/json", body).await
        {
            plugin_log.push_err("write response fail");
        }
    }
}

#[async_trait]
impl RequestFilter for GlobalAccessControl {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Kill switch.
        if !self.config.enable {
            plugin_log.push("disabled:pass");
            return PluginRunningResult::GoodNext;
        }
        // Fail OPEN on init failure (operator decision): allow + record loudly.
        if self.init_failed {
            plugin_log.push_err("init fail: fail-open");
            return PluginRunningResult::GoodNext;
        }

        let profile_name = self.resolve_active_profile_name(plugin_log);
        let Some(profile) = self.config.profiles.get(&profile_name) else {
            // Post-validation this should be unreachable; fail open if it ever happens.
            plugin_log.push_err("profile missing: fail-open");
            return PluginRunningResult::GoodNext;
        };

        // Plugin-level short circuit (BLOCKER 2 of the 2026-08-15 review): a
        // `failOpen: false` rule must reject "regardless of mode and
        // regardless of any other rule group" (design spec §5.2), independent
        // of GAC's usual fail-open-on-init-error/missing-profile posture.
        // Checking this via the group walk in `evaluate_profile` below is
        // order-dependent — an earlier group already matching, or an earlier
        // AND predicate in the geo rule's own group being false, both stop
        // the walk before the fail-closed geo rule is ever reached — so it is
        // hoisted here, before any group of the *resolved active* profile is
        // evaluated.
        if self.has_fail_closed_geo_rule && !crate::geoip::GeoIpProvider::global().is_loaded() {
            self.respond_geo_fail_closed(session, plugin_log).await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        let client_ip = self.get_client_ip(session);

        // Tri-state geo handling: a `FailClosed` group rejects the request
        // immediately, regardless of `default_action` — GAC's usual
        // fail-open-on-init-error posture does not apply to a rule the
        // operator explicitly wrote as `failOpen: false`.
        //
        // `GacGroupOutcome::FailClosed` is kept as defence-in-depth rather
        // than deleted, for the same reason as
        // `request_restriction::GroupOutcome::FailClosed`: it is a sound,
        // provably-equivalent fallback for every path that reaches here
        // today (see that plugin's comment), and it costs nothing to keep as
        // a guard against a future desync.
        let outcome = {
            let s: &dyn PluginSession = session;
            match evaluate_profile(profile, client_ip.as_ref(), s, self.geo_fail_open) {
                Ok(outcome) => outcome,
                Err(()) => {
                    self.respond_geo_fail_closed(session, plugin_log).await;
                    return PluginRunningResult::ErrTerminateRequest;
                }
            }
        };

        match outcome {
            AllowOutcome::Allowed { group } => {
                match group {
                    Some(g) => plugin_log.push(&format!("allow:group={}", g)),
                    None => plugin_log.push("allow:default"),
                };
                PluginRunningResult::GoodNext
            }
            AllowOutcome::Denied => {
                plugin_log.push("deny:no-match");
                let message = self.config.message.as_deref().unwrap_or("Access denied");
                let body = Bytes::from(serde_json::json!({ "message": message }).to_string());
                if let Err(_e) =
                    send_gateway_plain_error_response(session, self.config.status, "application/json", body).await
                {
                    plugin_log.push_err("write response fail");
                }
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
    use edgion_resources::resources::edgion_config_data::IpGroup;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        AllowProfile, AllowRuleGroup, GlobalAccessControlConfig,
    };
    use edgion_resources::resources::edgion_plugins::plugin_configs::{RestrictionRule, RestrictionSource};
    use edgion_resources::resources::edgion_plugins::DefaultAction;
    use std::collections::HashMap;

    fn ipg(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.into(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn secret_header_rule(key: &str, value: &str) -> RestrictionRule {
        RestrictionRule {
            source: RestrictionSource::Header,
            key: Some(key.into()),
            values: Some(vec![value.into()]),
            ..Default::default()
        }
    }

    // A lockdown config: cloudflare group = CIDR ∧ header. Base "normal" profile = allow-all.
    fn lockdown_config(active_profile_ref: Option<EdgionConfigDataRef>) -> GlobalAccessControlConfig {
        GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([
                (
                    "normal".to_string(),
                    AllowProfile {
                        default_action: DefaultAction::Allow,
                        rule_groups: vec![],
                    },
                ),
                (
                    "lockdown".to_string(),
                    AllowProfile {
                        default_action: DefaultAction::Deny,
                        rule_groups: vec![AllowRuleGroup {
                            name: Some("cloudflare".into()),
                            ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
                            rules: vec![secret_header_rule("X-CF-Secret", "tok")],
                            ip_matcher: None,
                        }],
                    },
                ),
            ]),
            description: None,
            active_profile_ref,
            status: 403,
            message: None,
            ip_source: edgion_resources::resources::edgion_plugins::IpSource::RemoteAddr,
            fail_open: None,
        }
    }

    #[tokio::test]
    async fn allow_when_cidr_and_secret_match() {
        let plugin = GlobalAccessControl::create(&lockdown_config(None), "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("173.245.48.5".to_string());
        s.expect_header_value().returning(|name| {
            if name == "X-CF-Secret" {
                Some("tok".to_string())
            } else {
                None
            }
        });
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("allow:group=cloudflare"));
    }

    #[tokio::test]
    async fn deny_when_cidr_matches_but_secret_absent() {
        let plugin = GlobalAccessControl::create(&lockdown_config(None), "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("173.245.48.5".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("deny:no-match"));
    }

    #[tokio::test]
    async fn deny_when_secret_matches_but_ip_outside_cidr() {
        let plugin = GlobalAccessControl::create(&lockdown_config(None), "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|name| {
            if name == "X-CF-Secret" {
                Some("tok".to_string())
            } else {
                None
            }
        });
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
    }

    #[tokio::test]
    async fn disabled_passes_through() {
        let mut cfg = lockdown_config(None);
        cfg.enable = false;
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("disabled:pass"));
    }

    #[tokio::test]
    async fn init_failure_fails_open() {
        // A config with an invalid CIDR fails validate_and_init at construction.
        let mut cfg = lockdown_config(None);
        cfg.profiles.get_mut("lockdown").unwrap().rule_groups[0].ip = Some(vec![ipg("bad", &["not-a-cidr"])]);
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        // Fail-OPEN: allow + record.
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("init fail: fail-open"));
    }

    #[tokio::test]
    async fn unresolved_selector_falls_back_to_base_active_profile() {
        // active_profile_ref points at an EdgionConfigData not present in the global store
        // (empty in unit tests) -> fail-to-base to active_profile "lockdown".
        let cfg = lockdown_config(Some(EdgionConfigDataRef {
            name: "ghost".into(),
            namespace: Some("edgion-system".into()),
            ..Default::default()
        }));
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        // lockdown base: IP outside + no header -> deny.
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        // The unresolvable selector (not in the empty store) must be recorded.
        assert!(log.contains("selector miss: base"));
    }

    /// A denied active_profile_ref must not be resolved as a store key — even if a
    /// valid Selector at that key would switch to the "normal" (allow-all) profile.
    /// Effect: base "lockdown" profile is used (deny-all for 8.8.8.8).
    #[tokio::test]
    async fn denied_active_profile_ref_falls_back_to_base() {
        use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
        use edgion_resources::resources::edgion_config_data::{
            ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, Selector,
        };
        use std::sync::Arc as StdArc;

        // Seed a valid Selector that WOULD switch to "normal" (allow-all).
        let ns = "gac-ref-denied-unit";
        let sel_key = format!("{ns}/sel-denied");
        let mut sel_ed = EdgionConfigData::new(
            "sel-denied",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("normal".to_string()),
                    description: None,
                }),
            },
        );
        sel_ed.metadata.namespace = Some(ns.to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(&sel_key, StdArc::new(sel_ed));

        // Config points at the Selector but marks it as denied.
        let cfg = lockdown_config(Some(EdgionConfigDataRef {
            name: "sel-denied".into(),
            namespace: Some(ns.into()),
            ref_denied: Some(edgion_resources::resources::common::RefDenied {
                target_namespace: ns.into(),
                target_name: "sel-denied".into(),
                reason: None,
            }),
        }));
        let plugin = GlobalAccessControl::create(&cfg, ns);
        let mut s = MockPluginSession::new();
        // With denied ref → active_profile_ref_key is None → base "lockdown" used → deny.
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(
            r,
            PluginRunningResult::ErrTerminateRequest,
            "denied active_profile_ref must not switch to normal (allow-all); lockdown must deny"
        );
        // No "selector miss" emitted — the key was never looked up (denied at construction).
        assert!(
            !log.contains("selector miss: base"),
            "denied ref must not trigger a store lookup"
        );
    }

    // The four tests below are ported from the now-deleted
    // `edgion-resources::global_access_control::tests` evaluate()-based suite
    // (Task 10 review: `AllowProfile::evaluate`/`AllowRuleGroup::matches` were
    // deleted as dead code once the gateway's byte-faithful port became the
    // only caller). They exercise behavior the gateway-side evaluator must
    // still guarantee that the pre-existing gateway tests above did not cover:
    // OR-across-groups, a permissive `default_action: Allow` base profile, a
    // group with no IP precondition, and fail-closed on an unknown client IP.

    /// OR across groups: a second group's CIDR+header pair also allows.
    #[tokio::test]
    async fn second_group_or_matches() {
        let mut cfg = lockdown_config(None);
        cfg.profiles
            .get_mut("lockdown")
            .unwrap()
            .rule_groups
            .push(AllowRuleGroup {
                name: Some("cloudfront".into()),
                ip: Some(vec![ipg("cfront", &["120.52.22.96/27"])]),
                rules: vec![secret_header_rule("X-Origin-Verify", "v2")],
                ip_matcher: None,
            });
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("120.52.22.100".to_string());
        s.expect_header_value().returning(|name| {
            if name == "X-Origin-Verify" {
                Some("v2".to_string())
            } else {
                None
            }
        });
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("allow:group=cloudfront"));
    }

    /// `default_action: Allow` with empty `rule_groups` (the base "normal"
    /// profile in `lockdown_config`) passes everything through.
    #[tokio::test]
    async fn base_profile_default_allow_passes_everything() {
        let mut cfg = lockdown_config(None);
        cfg.active_profile = "normal".into();
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|_| None);
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("allow:default"));
    }

    /// A group with no IP constraint must be evaluable when the client IP is
    /// unknown (unparseable/empty `client_addr`).
    #[tokio::test]
    async fn header_only_group_matches_without_client_ip() {
        let mut cfg = lockdown_config(None);
        cfg.profiles.get_mut("lockdown").unwrap().rule_groups = vec![AllowRuleGroup {
            name: Some("header-only".into()),
            ip: None,
            rules: vec![secret_header_rule("X-CF-Secret", "tok")],
            ip_matcher: None,
        }];
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const(String::new()); // unparseable -> client IP unknown
        s.expect_header_value().returning(|name| {
            if name == "X-CF-Secret" {
                Some("tok".to_string())
            } else {
                None
            }
        });
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("allow:group=header-only"));
    }

    /// An IP-constrained group must fail closed when the client IP cannot be
    /// determined, even though the credential header matches.
    #[tokio::test]
    async fn ip_constrained_group_denies_when_client_ip_unknown() {
        let cfg = lockdown_config(None); // single cloudflare group with an ip matcher
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const(String::new());
        s.expect_header_value().returning(|name| {
            if name == "X-CF-Secret" {
                Some("tok".to_string())
            } else {
                None
            }
        });
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("deny:no-match"));
    }

    fn geo_rule(source: RestrictionSource, key: &str) -> RestrictionRule {
        RestrictionRule {
            source,
            key: Some(key.into()),
            values: Some(vec!["RU".into()]),
            ..Default::default()
        }
    }

    /// GAC normally fails OPEN on init failure / missing profile, but a
    /// `failOpen: false` geo rule is an explicit operator ask and must
    /// still reject — that fail-open posture does not extend to it.
    #[tokio::test]
    async fn geo_fail_closed_rejects_even_though_gac_normally_fails_open() {
        use crate::geoip::{GeoIpProvider, GeoLookup, GEO_TEST_LOCK};
        {
            let _guard = GEO_TEST_LOCK.lock();
            GeoIpProvider::global().clear();
        }

        let mut cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Allow,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("geo".into()),
                        ip: None,
                        rules: vec![geo_rule(RestrictionSource::DirectGeo, "country")],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
            fail_open: Some(false),
        };
        cfg.validate_and_init().unwrap();

        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_direct_geo().returning(|| GeoLookup::Unavailable);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        s.expect_add_error().returning(|_| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(
            r,
            PluginRunningResult::ErrTerminateRequest,
            "failOpen:false must reject even though default_action is Allow"
        );
    }

    /// BLOCKER 2 regression: an earlier group already matching (which would
    /// normally ALLOW under `default_action: Allow`, and short-circuit the OR
    /// walk before reaching a later group at all) must not bypass a later
    /// group's `failOpen: false` rule.
    #[tokio::test]
    async fn fail_closed_geo_rule_in_a_later_group_still_rejects_even_though_an_earlier_group_matched() {
        use crate::geoip::{GeoIpProvider, GEO_TEST_LOCK};
        {
            let _guard = GEO_TEST_LOCK.lock();
            GeoIpProvider::global().clear();
        }

        let mut cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Allow,
                    rule_groups: vec![
                        AllowRuleGroup {
                            name: Some("header-match".into()),
                            ip: None,
                            rules: vec![secret_header_rule("X-Scanner", "1")],
                            ip_matcher: None,
                        },
                        AllowRuleGroup {
                            name: Some("geo".into()),
                            ip: None,
                            rules: vec![geo_rule(RestrictionSource::DirectGeo, "country")],
                            ip_matcher: None,
                        },
                    ],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
            fail_open: Some(false),
        };
        cfg.validate_and_init().unwrap();

        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|name| {
            if name == "X-Scanner" {
                Some("1".to_string())
            } else {
                None
            }
        });
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        s.expect_add_error().returning(|_| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(
            r,
            PluginRunningResult::ErrTerminateRequest,
            "failOpen:false in a later group must reject even though an earlier group already matched"
        );
    }

    /// BLOCKER 2 regression: the geo rule is not the first predicate in its
    /// own AND group. Before the plugin-level short circuit, `group_outcome`
    /// returned `NotMatched` on the earlier false header predicate before
    /// ever reaching the fail-closed geo rule.
    #[tokio::test]
    async fn fail_closed_geo_rule_not_first_in_its_and_group_still_rejects() {
        use crate::geoip::{GeoIpProvider, GEO_TEST_LOCK};
        {
            let _guard = GEO_TEST_LOCK.lock();
            GeoIpProvider::global().clear();
        }

        let mut cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("and-group".into()),
                        ip: None,
                        rules: vec![
                            secret_header_rule("X-Present", "only-this-value"),
                            geo_rule(RestrictionSource::DirectGeo, "country"),
                        ],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
            fail_open: Some(false),
        };
        cfg.validate_and_init().unwrap();

        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        // Header present but does NOT match the required value, so the first
        // predicate in the AND group is false.
        s.expect_header_value().returning(|name| {
            if name == "X-Present" {
                Some("something-else".to_string())
            } else {
                None
            }
        });
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        s.expect_add_error().returning(|_| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(
            r,
            PluginRunningResult::ErrTerminateRequest,
            "failOpen:false must reject even when an earlier AND predicate in its own group is false"
        );
    }

    /// `ip_source: RemoteAddr` (client_addr) paired with a `ClientGeo` rule
    /// (remote_addr) decides its AND from two different addresses; the
    /// disagreement check must flag it.
    #[test]
    fn ip_source_geo_disagreement_is_flagged() {
        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("cloudflare".into()),
                        ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
                        rules: vec![geo_rule(RestrictionSource::ClientGeo, "country")],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
            fail_open: Some(false),
        };
        let warnings = ip_source_geo_disagreements(&cfg);
        assert_eq!(warnings.len(), 1, "got: {:?}", warnings);
        assert!(warnings[0].contains("cloudflare"));
    }

    /// The agreeing pair (`RemoteAddr` + `DirectGeo`) must not be flagged.
    #[test]
    fn ip_source_geo_agreement_is_not_flagged() {
        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("cloudflare".into()),
                        ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
                        rules: vec![geo_rule(RestrictionSource::DirectGeo, "country")],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
            fail_open: Some(false),
        };
        assert!(ip_source_geo_disagreements(&cfg).is_empty());
    }
}

//! GlobalConnectionIpRestriction — L4 TCP connection-phase IP restriction (stream plugin config).
//!
//! This is a plain, git-owned plugin config: it carries the complete IP rule data
//! (`enable` + named `profiles`) inline. The only dynamic, hot-swappable input is *which
//! profile is active*, supplied via an optional `activeProfileRef` pointing at a `Selector`
//! EdgionConfigData. The rule data itself is never overlaid from outside.
//!
//! # Active-profile resolution (gateway, per-connection)
//! - No `activeProfileRef` → use `active_profile`.
//! - Selector present with `active: Some(name)` and `name` ∈ `profiles` → use `name`.
//! - Any error / `Selector.active: None` / envelope `enable=false` → fall back to
//!   `active_profile` (fail-to-base). A selector can only switch among these git-owned
//!   profiles; it can never disable filtering.

use crate::matcher::ip_cidr_helpers::{build_grouped_matcher_partial, evaluate_ip_access, EntryError};
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;

/// Complete, git-owned GlobalConnectionIpRestriction plugin config.
///
/// # Security invariant
/// This config MUST be authored as the conservative/safe default (e.g. `defaultAction: Deny`
/// plus an explicit allowlist). The active-profile selector falls back to `active_profile`
/// on any error, so a permissive default creates a wide-open fallback.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalConnectionIpRestrictionConfig {
    /// Emergency kill switch. When `false`, the data plane completely skips this plugin
    /// (allow all connections).
    pub enable: bool,

    /// Currently active profile key. Must exist in `profiles`. May be overridden at
    /// connection time by an `activeProfileRef` → `Selector` EdgionConfigData.
    pub active_profile: String,

    /// Named rule presets. Switch the active profile to swap rules instantly.
    pub profiles: HashMap<String, ProfileRules>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional reference to a `Selector` EdgionConfigData that supplies the *active
    /// profile* at connection time. Namespace defaults to the parent EdgionStreamPlugins
    /// namespace. When absent, `active_profile` is used directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<IpGroup>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<IpGroup>>,

    #[serde(default)]
    pub default_action: DefaultAction,

    /// Allow-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_refs: Option<Vec<EdgionConfigDataRef>>,
    /// Deny-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_refs: Option<Vec<EdgionConfigDataRef>>,

    // Runtime-only, not serialized
    #[serde(skip)]
    #[schemars(skip)]
    pub allow_matcher: Option<IpRadixMatcher>,
    #[serde(skip)]
    #[schemars(skip)]
    pub deny_matcher: Option<IpRadixMatcher>,
}

impl GlobalConnectionIpRestrictionConfig {
    /// Validate the config and pre-compile runtime matchers for every profile.
    /// Fails fast on the first invalid profile. `active_profile_ref` is not validated
    /// here (it is a controller-resolved cross-resource reference).
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        if self.profiles.is_empty() {
            return Err("GlobalConnectionIpRestriction requires at least one profile".to_string());
        }

        if !self.profiles.contains_key(&self.active_profile) {
            return Err(format!(
                "activeProfile '{}' not found in profiles {:?}",
                self.active_profile,
                self.profiles.keys().collect::<Vec<_>>()
            ));
        }

        for (profile_name, profile) in self.profiles.iter_mut() {
            profile
                .validate_and_init()
                .map_err(|e| format!("profile '{}': {}", profile_name, e))?;
        }

        Ok(())
    }
}

impl ProfileRules {
    pub(crate) fn validate_and_init(&mut self) -> Result<(), String> {
        let has_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let has_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !has_allow && !has_deny {
            return Err("at least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }

        // Intentionally stricter than HTTP/L4 IpRestriction: a present-but-empty
        // `allow`/`deny` (`Some(vec![])`) is REJECTED here (structural), preserving
        // GlobalConnection semantics — do not "harmonize" it into the `!is_empty()`
        // guard the other consumers use. A non-empty list is partial-good: bad CIDRs
        // are skipped, not fatal.
        if let Some(allow) = &self.allow {
            if allow.is_empty() {
                return Err("allow group list is empty".to_string());
            }
            let (m, _errs) = build_flat_matcher_partial(allow);
            self.allow_matcher = Some(m);
        }
        if let Some(deny) = &self.deny {
            if deny.is_empty() {
                return Err("deny group list is empty".to_string());
            }
            let (m, _errs) = build_flat_matcher_partial(deny);
            self.deny_matcher = Some(m);
        }
        use crate::matcher::ip_cidr_helpers::validate_refs_structure;
        if let Some(refs) = &self.allow_refs {
            validate_refs_structure(refs)?;
        }
        if let Some(refs) = &self.deny_refs {
            validate_refs_structure(refs)?;
        }
        Ok(())
    }

    /// Returns the full access decision (group name included).
    ///
    /// Priority: deny > allow > default_action. See spec §4.6.
    /// Delegates to the shared `evaluate_ip_access` so the L4, HTTP, and
    /// global-connection IP-restriction paths cannot drift.
    pub fn check_ip(&self, ip: &IpAddr) -> AccessDecision {
        evaluate_ip_access(
            self.deny_matcher.as_ref(),
            self.allow_matcher.as_ref(),
            &self.default_action,
            ip,
        )
    }

    /// Whitelist mode by configured intent (spec Decision 5).
    pub fn allow_mode(&self) -> bool {
        self.allow.as_ref().is_some_and(|v| !v.is_empty()) || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// Decision over inline ∪ gateway-resolved reference matchers.
    pub fn check_ip_with_refs(
        &self,
        ip: &IpAddr,
        deny_ref_matchers: &[&IpRadixMatcher],
        allow_ref_matchers: &[&IpRadixMatcher],
    ) -> AccessDecision {
        use crate::matcher::ip_cidr_helpers::evaluate_ip_access_multi;
        let mut deny: Vec<&IpRadixMatcher> = self.deny_matcher.iter().collect();
        deny.extend_from_slice(deny_ref_matchers);
        let mut allow: Vec<&IpRadixMatcher> = self.allow_matcher.iter().collect();
        allow.extend_from_slice(allow_ref_matchers);
        evaluate_ip_access_multi(&deny, &allow, self.allow_mode(), &self.default_action, ip)
    }
}

/// Partial-good flat matcher builder. Used by `ProfileRules` once the
/// present-but-empty structural guard (kept in `validate_and_init`) has passed:
/// it flattens `Vec<IpGroup>` into a single IpRadixMatcher over the VALID entries,
/// skipping (and returning) bad CIDRs rather than failing the whole profile.
fn build_flat_matcher_partial(groups: &[IpGroup]) -> (IpRadixMatcher, Vec<EntryError>) {
    build_grouped_matcher_partial(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== helpers =====

    fn valid_profile(name: &str) -> (String, ProfileRules) {
        (
            name.to_string(),
            ProfileRules {
                allow: Some(vec![IpGroup {
                    name: "office".to_string(),
                    description: None,
                    cidrs: vec!["192.168.1.0/24".to_string()],
                }]),
                deny: None,
                default_action: DefaultAction::Deny,
                allow_refs: None,
                deny_refs: None,
                allow_matcher: None,
                deny_matcher: None,
            },
        )
    }

    #[test]
    fn construction_and_serde_roundtrip() {
        let (k, v) = valid_profile("p1");
        let data = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([(k, v)]),
            description: Some("test".to_string()),
            active_profile_ref: None,
        };
        let j = serde_json::to_value(&data).unwrap();
        let back: GlobalConnectionIpRestrictionConfig = serde_json::from_value(j).unwrap();
        assert_eq!(back.active_profile, "p1");
        assert!(back.enable);
    }

    fn empty_profile() -> ProfileRules {
        ProfileRules {
            allow: None,
            deny: None,
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        }
    }

    fn group(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.to_string(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn validate_empty_profiles_errs() {
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::new(),
            description: None,
            active_profile_ref: None,
        };
        let err = d.validate_and_init().unwrap_err();
        assert!(err.contains("at least one profile"), "{}", err);
    }

    #[test]
    fn validate_active_profile_must_exist_errs() {
        let (_, p) = valid_profile("p1");
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "nope".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        let err = d.validate_and_init().unwrap_err();
        assert!(err.contains("activeProfile") && err.contains("nope"), "{}", err);
    }

    #[test]
    fn validate_profile_requires_allow_or_deny() {
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), empty_profile())]),
            description: None,
            active_profile_ref: None,
        };
        let err = d.validate_and_init().unwrap_err();
        assert!(err.contains("at least one of") && err.contains("allow"), "{}", err);
    }

    #[test]
    fn validate_duplicate_group_names_is_partial_good() {
        // Partial-good: a duplicate group name is keep-first (the later occurrence is
        // skipped, not fatal). The first "office" group's CIDR survives.
        let p = ProfileRules {
            allow: Some(vec![
                group("office", &["192.168.1.0/24"]),
                group("office", &["10.0.0.0/8"]),
            ]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        assert!(d.validate_and_init().is_ok());
        let p = d.profiles.get("p1").unwrap();
        // First occurrence ("office" with 192.168.1.0/24) survives.
        assert!(is_allowed(p.check_ip(&"192.168.1.50".parse().unwrap())));
        // Second occurrence's CIDR (10.0.0.0/8) was skipped -> whitelist miss.
        assert!(matches!(
            p.check_ip(&"10.0.0.1".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn validate_empty_cidrs_group_is_partial_good() {
        // Partial-good: an inline group with no cidrs is now SKIPPED (not fatal) because
        // the list itself is present-and-non-empty. Only a present-but-empty group LIST
        // (`allow: Some(vec![])`) is still rejected (see validate_allow_some_empty_errs_cleanly).
        let p = ProfileRules {
            allow: Some(vec![group("office", &[])]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        assert!(d.validate_and_init().is_ok());
        assert!(d.profiles.get("p1").unwrap().allow_matcher.is_some());
    }

    #[test]
    fn validate_bad_cidr_is_partial_good() {
        // Partial-good (CIDR-only leniency): a bad inline CIDR is skipped, not fatal.
        let p = ProfileRules {
            allow: Some(vec![group("office", &["not-an-ip", "10.0.0.0/8"])]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        assert!(d.validate_and_init().is_ok());
        // The good CIDR survived.
        let p = d.profiles.get("p1").unwrap();
        assert!(is_allowed(p.check_ip(&"10.1.2.3".parse().unwrap())));
    }

    #[test]
    fn validate_present_but_empty_allow_list_still_errs() {
        // Structural guard kept: a present-but-empty allow group LIST is rejected.
        // A valid `deny` is supplied so the at-least-one guard passes and we exercise
        // the empty-allow-list path specifically.
        let mut p = ProfileRules {
            allow: Some(vec![]),
            deny: Some(vec![group("bad", &["1.2.3.4"])]),
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        assert_eq!(p.validate_and_init(), Err("allow group list is empty".to_string()));
    }

    #[test]
    fn validate_ok_builds_matchers() {
        let (k, v) = valid_profile("p1");
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([(k, v)]),
            description: None,
            active_profile_ref: None,
        };
        d.validate_and_init().unwrap();
        let p = d.profiles.get("p1").unwrap();
        assert!(p.allow_matcher.is_some());
    }

    fn init(data: &mut GlobalConnectionIpRestrictionConfig) {
        data.validate_and_init().unwrap();
    }

    /// Maps an `AccessDecision` to a boolean allow/deny for test assertions.
    fn is_allowed(decision: AccessDecision) -> bool {
        matches!(
            decision,
            AccessDecision::AllowByGroup(_) | AccessDecision::Default(DefaultAction::Allow)
        )
    }

    /// Convenience: any kind of deny decision.
    fn is_denied(decision: AccessDecision) -> bool {
        matches!(
            decision,
            AccessDecision::DenyByGroup(_)
                | AccessDecision::DenyByWhitelistMiss
                | AccessDecision::Default(DefaultAction::Deny)
        )
    }

    #[test]
    fn check_ip_allow_only_whitelist_semantics() {
        let p = ProfileRules {
            allow: Some(vec![group("office", &["192.168.1.0/24"])]),
            deny: None,
            default_action: DefaultAction::Allow, // irrelevant when allow is present
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        init(&mut d);
        let p = d.profiles.get("p1").unwrap();
        assert!(is_allowed(p.check_ip(&"192.168.1.50".parse().unwrap())));
        // Whitelist mode: unmatched IP -> DenyByWhitelistMiss.
        assert!(matches!(
            p.check_ip(&"8.8.8.8".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn check_ip_deny_wins_over_allow() {
        let p = ProfileRules {
            allow: Some(vec![group("all", &["10.0.0.0/8"])]),
            deny: Some(vec![group("bad", &["10.0.0.100"])]),
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        init(&mut d);
        let p = d.profiles.get("p1").unwrap();
        // 10.0.0.1 matches allow group "all".
        assert!(is_allowed(p.check_ip(&"10.0.0.1".parse().unwrap())));
        // 10.0.0.100 matches deny group "bad" (wins over allow).
        match p.check_ip(&"10.0.0.100".parse().unwrap()) {
            AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "bad"),
            other => panic!("expected DenyByGroup(bad), got {:?}", other),
        }
        // 8.8.8.8 outside allow -> DenyByWhitelistMiss.
        assert!(matches!(
            p.check_ip(&"8.8.8.8".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn check_ip_deny_only_default_allow() {
        let p = ProfileRules {
            allow: None,
            deny: Some(vec![group("bad", &["1.2.3.4"])]),
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        init(&mut d);
        let p = d.profiles.get("p1").unwrap();
        assert!(is_denied(p.check_ip(&"1.2.3.4".parse().unwrap())));
        assert!(is_allowed(p.check_ip(&"8.8.8.8".parse().unwrap())));
    }

    #[test]
    fn check_ip_multi_group_union() {
        let p = ProfileRules {
            allow: Some(vec![
                group("cloudflare", &["173.245.48.0/20"]),
                group("office", &["192.168.1.0/24"]),
            ]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        init(&mut d);
        let p = d.profiles.get("p1").unwrap();
        // 173.245.48.1 matches the cloudflare group.
        match p.check_ip(&"173.245.48.1".parse().unwrap()) {
            AccessDecision::AllowByGroup(g) => assert_eq!(&*g, "cloudflare"),
            other => panic!("expected AllowByGroup(cloudflare), got {:?}", other),
        }
        // 192.168.1.50 matches the office group.
        match p.check_ip(&"192.168.1.50".parse().unwrap()) {
            AccessDecision::AllowByGroup(g) => assert_eq!(&*g, "office"),
            other => panic!("expected AllowByGroup(office), got {:?}", other),
        }
        // Outside any allow group -> DenyByWhitelistMiss.
        assert!(matches!(
            p.check_ip(&"8.8.8.8".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn validate_allow_some_empty_errs_cleanly() {
        let p = ProfileRules {
            allow: Some(vec![]), // Some but empty
            deny: Some(vec![group("bad", &["1.2.3.4"])]),
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        let err = d.validate_and_init().unwrap_err();
        // Should error cleanly mentioning empty group list (not opaque "Failed to build IpRadixMatcher")
        assert!(err.contains("empty"), "error should mention empty: {}", err);
    }

    #[test]
    fn profile_allow_mode_true_when_only_allow_refs_set() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let p = ProfileRules {
            allow: None,
            deny: None,
            default_action: DefaultAction::Deny,
            allow_refs: Some(vec![EdgionConfigDataRef {
                name: "corp".into(),
                namespace: None,
                ..Default::default()
            }]),
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        assert!(p.allow_mode());
    }

    #[test]
    fn profile_refs_only_passes_validate_and_init() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let p = ProfileRules {
            allow: None,
            deny: None,
            default_action: DefaultAction::Deny,
            allow_refs: None,
            deny_refs: Some(vec![EdgionConfigDataRef {
                name: "blk".into(),
                namespace: None,
                ..Default::default()
            }]),
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
            active_profile_ref: None,
        };
        assert!(d.validate_and_init().is_ok());
    }

    #[test]
    fn profile_with_refs_deny_union_blocks_referenced_ip() {
        use crate::matcher::ip_cidr_helpers::build_grouped_matcher;
        let mut p = ProfileRules {
            allow: None,
            deny: Some(vec![group("inline", &["1.1.1.1"])]),
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        p.validate_and_init().unwrap();
        let ref_deny = build_grouped_matcher(&[group("reflist", &["2.2.2.2"])]).unwrap();
        assert!(matches!(
            p.check_ip_with_refs(&"2.2.2.2".parse().unwrap(), &[&ref_deny], &[]),
            AccessDecision::DenyByGroup(_)
        ));
        assert!(is_allowed(p.check_ip_with_refs(
            &"8.8.8.8".parse().unwrap(),
            &[&ref_deny],
            &[]
        )));
    }
}

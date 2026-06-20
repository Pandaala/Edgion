//! GlobalAccessControl plugin configuration.
//!
//! An L7 allow-list plugin for origin/CDN lockdown. Each `AllowProfile` holds OR-ed
//! `AllowRuleGroup`s; a group matches when its optional IP/CIDR precondition holds AND
//! all of its request-attribute `rules` hold (AND). Any matching group => allow; no
//! match => the profile's `default_action`. Profiles are hot-swappable via an
//! `activeProfileRef` Selector (resolved by the gateway; fail-to-base to `active_profile`).
//!
//! Logic-vs-data split mirrors `request_restriction`: this layer is session-free. The
//! gateway extracts source values and passes them in via the `evaluate` accessor closure.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;

use super::ip_restriction::DefaultAction;
use super::ip_restriction::IpSource;
use super::request_match::{OnMissing, RestrictionRule};
use crate::matcher::ip_cidr_helpers::{build_grouped_matcher, evaluate_ip_access, validate_groups_structure};
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::{AccessDecision, PluginValidation};

fn default_status() -> u16 {
    403
}

fn default_ip_source() -> IpSource {
    // CDN/origin lockdown must match the true TCP peer, which cannot be forged.
    IpSource::RemoteAddr
}

/// GlobalAccessControl plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalAccessControlConfig {
    /// Emergency kill switch. When false the plugin passes everything through.
    pub enable: bool,

    /// Base/default profile key; must exist in `profiles`. May be overridden at request
    /// time by an `activeProfileRef` Selector. Used as fail-to-base on any selector error.
    pub active_profile: String,

    /// Named profiles (e.g. `normal` permissive base, `lockdown` restrictive).
    pub profiles: HashMap<String, AllowProfile>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional Selector EdgionConfigData supplying the active profile name at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,

    /// HTTP status for denial (default 403).
    #[serde(default = "default_status")]
    #[schemars(range(min = 100, max = 599))]
    pub status: u16,

    /// Optional denial body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// IP source. Defaults to RemoteAddr (TCP peer, un-forgeable). Only set ClientIp when a
    /// trusted L4 LB fronts the gateway and RealIp is correctly configured.
    #[serde(default = "default_ip_source")]
    pub ip_source: IpSource,
}

/// A named profile: OR-ed allow groups plus the no-match action.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllowProfile {
    /// Action when no group matches: Allow (permissive pass-through base) or Deny (lockdown).
    #[serde(default)]
    pub default_action: DefaultAction,

    /// Allow groups, OR-ed. May be empty (with default_action: Allow => pass-through base).
    #[serde(default)]
    pub rule_groups: Vec<AllowRuleGroup>,
}

/// One allow group: optional IP/CIDR precondition AND request-attribute rules (AND).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllowRuleGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// CIDR precondition (radix). None = no IP constraint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<Vec<IpGroup>>,

    /// Request-attribute predicates, ANDed. Reuses the shared RestrictionRule engine.
    #[serde(default)]
    pub rules: Vec<RestrictionRule>,

    // Runtime-only compiled IP matcher.
    #[serde(skip)]
    #[schemars(skip)]
    pub ip_matcher: Option<IpRadixMatcher>,
}

/// Result of evaluating a profile against a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowOutcome {
    /// Allowed. `group` = matched group name, or `None` when allowed by default_action.
    Allowed { group: Option<String> },
    /// Denied (no group matched and default_action is Deny).
    Denied,
}

impl GlobalAccessControlConfig {
    /// Validate and compile every profile's IP matchers and rule matchers.
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        if self.profiles.is_empty() {
            return Err("globalAccessControl: at least one profile is required".to_string());
        }
        if !self.profiles.contains_key(&self.active_profile) {
            return Err(format!(
                "globalAccessControl: activeProfile '{}' not found in profiles",
                self.active_profile
            ));
        }
        if self.status < 100 || self.status >= 600 {
            return Err(format!("globalAccessControl: invalid status code: {}", self.status));
        }
        for (pname, profile) in self.profiles.iter_mut() {
            profile
                .validate_and_init()
                .map_err(|e| format!("profile '{}': {}", pname, e))?;
        }
        Ok(())
    }

    /// Stateless validation mirror (no compile) for preparse-time ingest.
    pub fn check_fields(&self) -> Option<String> {
        if self.profiles.is_empty() {
            return Some("globalAccessControl: at least one profile is required".to_string());
        }
        if !self.profiles.contains_key(&self.active_profile) {
            return Some(format!(
                "globalAccessControl: activeProfile '{}' not found in profiles",
                self.active_profile
            ));
        }
        if self.status < 100 || self.status >= 600 {
            return Some(format!("globalAccessControl: invalid status code: {}", self.status));
        }
        for (pname, profile) in &self.profiles {
            if let Some(err) = profile.check_fields() {
                return Some(format!("profile '{}': {}", pname, err));
            }
        }
        None
    }

    /// Resolve a profile by optional selector name, returning None on unknown name so the
    /// caller can apply fail-to-base. `None` selector => the base `active_profile`.
    pub fn resolve_profile(&self, selected: Option<&str>) -> Option<&AllowProfile> {
        match selected {
            Some(name) => self.profiles.get(name),
            None => self.profiles.get(&self.active_profile),
        }
    }
}

impl AllowProfile {
    fn validate_and_init(&mut self) -> Result<(), String> {
        for (gi, group) in self.rule_groups.iter_mut().enumerate() {
            group.validate_and_init().map_err(|e| format!("group[{}]: {}", gi, e))?;
        }
        Ok(())
    }

    fn check_fields(&self) -> Option<String> {
        for (gi, group) in self.rule_groups.iter().enumerate() {
            if let Some(err) = group.check_fields() {
                return Some(format!("group[{}]: {}", gi, err));
            }
        }
        None
    }

    /// Evaluate the profile. `ip` is the client IP (None when undeterminable).
    /// `get_value(rule)` returns the extracted source value for a rule.
    pub fn evaluate(
        &self,
        ip: Option<&IpAddr>,
        get_value: &dyn Fn(&RestrictionRule) -> Option<String>,
    ) -> AllowOutcome {
        for group in &self.rule_groups {
            if group.matches(ip, get_value) {
                return AllowOutcome::Allowed {
                    group: group.name.clone(),
                };
            }
        }
        match self.default_action {
            DefaultAction::Allow => AllowOutcome::Allowed { group: None },
            DefaultAction::Deny => AllowOutcome::Denied,
        }
    }
}

impl AllowRuleGroup {
    fn validate_and_init(&mut self) -> Result<(), String> {
        if let Some(err) = self.check_fields() {
            return Err(err);
        }
        if let Some(groups) = &self.ip {
            if !groups.is_empty() {
                self.ip_matcher = Some(build_grouped_matcher(groups)?);
            }
        }
        for rule in self.rules.iter_mut() {
            rule.compile()?;
        }
        Ok(())
    }

    fn check_fields(&self) -> Option<String> {
        let has_ip = self.ip.as_ref().is_some_and(|g| !g.is_empty());
        if !has_ip && self.rules.is_empty() {
            return Some("an allow group must have a non-empty 'ip' or at least one rule".to_string());
        }
        if let Some(groups) = &self.ip {
            if !groups.is_empty() {
                if let Err(e) = validate_groups_structure(groups) {
                    return Some(format!("ip: {}", e));
                }
            }
        }
        for rule in &self.rules {
            if let Some(err) = rule.field_error() {
                return Some(err);
            }
            // A secretRef rule is a credential check: an absent header MUST deny. With
            // onMissing Match (absent => true) or Skip (absent => dropped from the AND),
            // a request could be allowed on the IP precondition alone, defeating the
            // credential and the whole origin-lockdown intent. Force NoMatch.
            if rule.secret_ref.is_some() && rule.on_missing != OnMissing::NoMatch {
                return Some(format!(
                    "rule '{}': a secretRef rule must use onMissing: NoMatch (Match/Skip would allow on IP alone, defeating the credential check)",
                    rule.display_name()
                ));
            }
        }
        None
    }

    /// True iff the IP precondition holds (or absent) AND every non-skipped rule holds.
    fn matches(&self, ip: Option<&IpAddr>, get_value: &dyn Fn(&RestrictionRule) -> Option<String>) -> bool {
        // Counts as evidence so an IP-only group matches on IP alone.
        let mut considered = false;
        if let Some(matcher) = &self.ip_matcher {
            // IP constraint present but client IP unknown => cannot verify => fail closed.
            let Some(ip) = ip else {
                return false;
            };
            let in_set = matches!(
                evaluate_ip_access(None, Some(matcher), &DefaultAction::Deny, ip),
                AccessDecision::AllowByGroup(_)
            );
            if !in_set {
                return false;
            }
            considered = true;
        }
        for rule in &self.rules {
            match rule.evaluate(get_value(rule).as_deref()) {
                Some(true) => considered = true,
                Some(false) => return false,
                None => {} // Skip: dropped from the AND
            }
        }
        considered
    }
}

impl PluginValidation for GlobalAccessControlConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::edgion_config_data::IpGroup;
    use crate::resources::edgion_plugins::plugin_configs::{RestrictionRule, RestrictionSource};
    use std::net::IpAddr;

    fn ipg(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.into(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn header_rule(key: &str, value: &str) -> RestrictionRule {
        RestrictionRule {
            source: RestrictionSource::Header,
            key: Some(key.into()),
            values: Some(vec![value.into()]),
            ..Default::default()
        }
    }

    fn lockdown() -> GlobalAccessControlConfig {
        let mut cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: std::collections::HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("cloudflare".into()),
                        ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
                        rules: vec![header_rule("X-CF-Secret", "tok")],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
        };
        cfg.validate_and_init().unwrap();
        cfg
    }

    // value accessor: only the matching header present
    fn acc<'a>(header: Option<(&'a str, &'a str)>) -> impl Fn(&RestrictionRule) -> Option<String> + 'a {
        move |rule: &RestrictionRule| {
            let want = rule.key.as_deref()?;
            header.and_then(|(k, v)| if k == want { Some(v.to_string()) } else { None })
        }
    }

    fn secret_rule(key: &str, on_missing: OnMissing) -> RestrictionRule {
        use crate::resources::gateway::SecretObjectReference;
        RestrictionRule {
            source: RestrictionSource::Header,
            key: Some(key.into()),
            secret_ref: Some(SecretObjectReference {
                name: "cf-origin-secret".into(),
                group: None,
                kind: None,
                namespace: None,
            }),
            on_missing,
            ..Default::default()
        }
    }

    #[test]
    fn secret_ref_rule_rejects_on_missing_skip() {
        // A Skip on a credential rule would allow on IP alone — must be rejected.
        let mut cfg = lockdown();
        cfg.profiles.get_mut("lockdown").unwrap().rule_groups = vec![AllowRuleGroup {
            name: Some("cloudflare".into()),
            ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
            rules: vec![secret_rule("X-CF-Secret", OnMissing::Skip)],
            ip_matcher: None,
        }];
        let err = cfg.validate_and_init().unwrap_err();
        assert!(err.contains("onMissing: NoMatch"), "got: {}", err);
    }

    #[test]
    fn secret_ref_rule_rejects_on_missing_match() {
        let mut cfg = lockdown();
        cfg.profiles.get_mut("lockdown").unwrap().rule_groups = vec![AllowRuleGroup {
            name: Some("cloudflare".into()),
            ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
            rules: vec![secret_rule("X-CF-Secret", OnMissing::Match)],
            ip_matcher: None,
        }];
        assert!(cfg.validate_and_init().is_err());
    }

    #[test]
    fn secret_ref_rule_accepts_on_missing_no_match() {
        let mut cfg = lockdown();
        cfg.profiles.get_mut("lockdown").unwrap().rule_groups = vec![AllowRuleGroup {
            name: Some("cloudflare".into()),
            ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
            rules: vec![secret_rule("X-CF-Secret", OnMissing::NoMatch)],
            ip_matcher: None,
        }];
        assert!(cfg.validate_and_init().is_ok());
    }

    #[test]
    fn allow_when_ip_and_header_both_match() {
        let cfg = lockdown();
        let p = cfg.resolve_profile(Some("lockdown")).unwrap();
        let ip: IpAddr = "173.245.48.5".parse().unwrap();
        let out = p.evaluate(Some(&ip), &acc(Some(("X-CF-Secret", "tok"))));
        assert!(matches!(out, AllowOutcome::Allowed { .. }));
    }

    #[test]
    fn deny_when_ip_matches_but_header_missing() {
        let cfg = lockdown();
        let p = cfg.resolve_profile(Some("lockdown")).unwrap();
        let ip: IpAddr = "173.245.48.5".parse().unwrap();
        let out = p.evaluate(Some(&ip), &acc(None));
        assert!(matches!(out, AllowOutcome::Denied));
    }

    #[test]
    fn deny_when_header_matches_but_ip_outside() {
        let cfg = lockdown();
        let p = cfg.resolve_profile(Some("lockdown")).unwrap();
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        let out = p.evaluate(Some(&ip), &acc(Some(("X-CF-Secret", "tok"))));
        assert!(matches!(out, AllowOutcome::Denied));
    }

    #[test]
    fn second_group_or_matches() {
        let mut cfg = lockdown();
        cfg.profiles
            .get_mut("lockdown")
            .unwrap()
            .rule_groups
            .push(AllowRuleGroup {
                name: Some("cloudfront".into()),
                ip: Some(vec![ipg("cfront", &["120.52.22.96/27"])]),
                rules: vec![header_rule("X-Origin-Verify", "v2")],
                ip_matcher: None,
            });
        cfg.validate_and_init().unwrap();
        let p = cfg.resolve_profile(Some("lockdown")).unwrap();
        let ip: IpAddr = "120.52.22.100".parse().unwrap();
        let out = p.evaluate(Some(&ip), &acc(Some(("X-Origin-Verify", "v2"))));
        assert!(matches!(out, AllowOutcome::Allowed { .. }));
    }

    #[test]
    fn base_profile_default_allow_passes_everything() {
        let p = AllowProfile {
            default_action: DefaultAction::Allow,
            rule_groups: vec![],
        };
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(matches!(
            p.evaluate(Some(&ip), &acc(None)),
            AllowOutcome::Allowed { .. }
        ));
    }

    #[test]
    fn resolve_profile_falls_back_to_active_on_unknown() {
        let cfg = lockdown();
        // Unknown selector name -> None (caller applies fail-to-base with active_profile).
        assert!(cfg.resolve_profile(Some("ghost")).is_none());
        // active_profile always resolves.
        assert!(cfg.resolve_profile(None).is_some());
    }

    #[test]
    fn validation_rejects_unknown_active_profile() {
        let mut cfg = lockdown();
        cfg.active_profile = "ghost".into();
        assert!(cfg.validate_and_init().is_err());
    }

    #[test]
    fn validation_rejects_empty_group() {
        let mut cfg = lockdown();
        cfg.profiles
            .get_mut("lockdown")
            .unwrap()
            .rule_groups
            .push(AllowRuleGroup {
                name: Some("empty".into()),
                ip: None,
                rules: vec![],
                ip_matcher: None,
            });
        assert!(cfg.validate_and_init().is_err());
    }

    #[test]
    fn header_only_group_matches_without_client_ip() {
        // A group with no IP constraint must be evaluable when the client IP is unknown.
        let mut cfg = lockdown();
        cfg.profiles.get_mut("lockdown").unwrap().rule_groups = vec![AllowRuleGroup {
            name: Some("header-only".into()),
            ip: None,
            rules: vec![header_rule("X-CF-Secret", "tok")],
            ip_matcher: None,
        }];
        cfg.validate_and_init().unwrap();
        let p = cfg.resolve_profile(Some("lockdown")).unwrap();
        let out = p.evaluate(None, &acc(Some(("X-CF-Secret", "tok"))));
        assert!(matches!(out, AllowOutcome::Allowed { .. }));
    }

    #[test]
    fn ip_constrained_group_denies_when_client_ip_unknown() {
        // An IP-constrained group must fail closed when the client IP cannot be determined.
        let cfg = lockdown(); // single cloudflare group with an ip matcher
        let p = cfg.resolve_profile(Some("lockdown")).unwrap();
        let out = p.evaluate(None, &acc(Some(("X-CF-Secret", "tok"))));
        assert!(matches!(out, AllowOutcome::Denied));
    }
}

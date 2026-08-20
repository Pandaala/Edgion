//! GlobalAccessControl plugin configuration.
//!
//! An L7 allow-list plugin for origin/CDN lockdown. Each `AllowProfile` holds OR-ed
//! `AllowRuleGroup`s; a group matches when its optional IP/CIDR precondition holds AND
//! all of its request-attribute `rules` hold (AND). Any matching group => allow; no
//! match => the profile's `default_action`. Profiles are hot-swappable via an
//! `activeProfileRef` Selector (resolved by the gateway; fail-to-base to `active_profile`).
//!
//! Logic-vs-data split mirrors `request_restriction`: this layer is session-free.
//! Config parsing/compilation (`validate_and_init`/`check_fields`) lives here; the
//! actual match/deny decision (formerly `AllowProfile::evaluate`/
//! `AllowRuleGroup::matches`) has moved to the gateway
//! (`edgion-gateway/src/plugins/http/global_access_control/plugin.rs`'s
//! `evaluate_profile`/`group_outcome`), which is the only caller and needs to
//! understand the gateway-only geo tri-state signal this crate cannot express.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ip_restriction::DefaultAction;
use super::ip_restriction::IpSource;
use super::request_match::{OnMissing, RestrictionRule};
use crate::matcher::ip_cidr_helpers::{build_grouped_matcher, validate_groups_structure};
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::PluginValidation;

fn default_status() -> u16 {
    403
}

fn default_ip_source() -> IpSource {
    // CDN/origin lockdown must match the true TCP peer, which cannot be forged.
    IpSource::DirectPeerIp
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

    /// IP source. Defaults to DirectPeerIp (raw TCP peer, un-forgeable). Only set RemoteIp when a
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
    ///
    /// Delegates the field-only checks to `check_fields()` first (rather than
    /// duplicating them) so the gateway's compile path and the controller's
    /// ingest path (`check_fields()`/`PluginValidation::validation_error`)
    /// are provably byte-identical on every rejection — including the new
    /// plugin-level `failOpen` check.
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        if let Some(err) = self.check_fields() {
            return Err(err);
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

    // The former `evaluate()` (profile-level match/deny decision) has moved to
    // the gateway: `edgion-gateway/src/plugins/http/global_access_control/
    // plugin.rs`'s `evaluate_profile`/`group_outcome` are a byte-faithful
    // port. Keeping both would have been two implementations of the same
    // security-control semantics with no caller left for this one — see Task
    // 10 review.
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

    // The former `matches()` (IP precondition AND rule evaluation for one
    // group) has moved to the gateway alongside `AllowProfile::evaluate` —
    // see the note there.
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
            ip_source: IpSource::DirectPeerIp,
        };
        cfg.validate_and_init().unwrap();
        cfg
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

    // The former evaluate()-based tests here (`allow_when_ip_and_header_both_match`,
    // `deny_when_ip_matches_but_header_missing`, `deny_when_header_matches_but_ip_outside`,
    // `second_group_or_matches`, `base_profile_default_allow_passes_everything`,
    // `header_only_group_matches_without_source_ip`,
    // `ip_constrained_group_denies_when_source_ip_unknown`) exercised
    // `AllowProfile::evaluate`/`AllowRuleGroup::matches`, which have been deleted
    // (no callers left — the gateway ported the logic; see the module doc). Their
    // coverage now lives in
    // `edgion-gateway/src/plugins/http/global_access_control/plugin.rs`'s test
    // module: `allow_when_cidr_and_secret_match`, `deny_when_cidr_matches_but_secret_absent`,
    // `deny_when_secret_matches_but_ip_outside_cidr` already existed there;
    // `second_group_or_matches`, `base_profile_default_allow_passes_everything`,
    // `header_only_group_matches_without_source_ip`, and
    // `ip_constrained_group_denies_when_source_ip_unknown` were ported as part of
    // this deletion (Task 10 review).

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
}

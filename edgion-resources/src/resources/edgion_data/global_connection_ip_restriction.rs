//! GlobalConnectionIpRestriction EdgionData variant — L4 TCP connection-phase IP restriction.
//!
//! Two related types live here:
//! - [`GlobalConnectionIpRestrictionData`] — the full, standalone config (still a valid `DataEntry`
//!   variant for compatibility and standalone EdgionData resources).
//! - [`GlobalConnIpRestrictionOverride`] — the all-Optional typed overlay consumed via `overrideRef`
//!   from `GlobalConnectionIpRestrictionEntry` in `EdgionStreamPlugins`.

use crate::matcher::ip_cidr_helpers::{build_grouped_matcher, evaluate_ip_access};
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;

// ===== PM data root =====

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalConnectionIpRestrictionData {
    /// Emergency kill switch. When `false`, the data plane completely skips this PM
    /// (equivalent to not referenced).
    pub enable: bool,

    /// Currently active profile key. Must exist in `profiles`.
    pub active_profile: String,

    /// Named rule presets. Switch `active_profile` to swap rules instantly.
    pub profiles: HashMap<String, ProfileRules>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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

    // Runtime-only, not serialized
    #[serde(skip)]
    #[schemars(skip)]
    pub allow_matcher: Option<IpRadixMatcher>,
    #[serde(skip)]
    #[schemars(skip)]
    pub deny_matcher: Option<IpRadixMatcher>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpGroup {
    /// Group name (unique within a profile). Used for admin organization; runtime flattens all groups.
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// CIDR strings or bare IPs (bare IPs are internally converted to /32 or /128).
    pub cidrs: Vec<String>,
}

impl GlobalConnectionIpRestrictionData {
    /// Validate the PM and pre-compile runtime matchers for every profile.
    /// Fails fast on the first invalid profile.
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
        let has_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty());
        let has_deny = self.deny.as_ref().is_some_and(|v| !v.is_empty());
        if !has_allow && !has_deny {
            return Err("at least one of 'allow' or 'deny' must be non-empty".to_string());
        }

        // Intentionally stricter than HTTP/Stream IpRestriction: a present
        // but empty `allow`/`deny` (`Some(vec![])`) is rejected here via
        // `build_flat_matcher` ("group list is empty"), rather than being
        // treated as absent. This preserves pre-existing GlobalConnection
        // semantics — do not "harmonize" it into the `!is_empty()` guard
        // the other consumers use.
        if let Some(allow) = &self.allow {
            self.allow_matcher = Some(build_flat_matcher(allow)?);
        }
        if let Some(deny) = &self.deny {
            self.deny_matcher = Some(build_flat_matcher(deny)?);
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
}

/// Flatten `Vec<IpGroup>` into a single IpRadixMatcher.
/// Delegates to the shared grouped-matcher builder, which performs name
/// validation (charset + length), name uniqueness, cross-group CIDR
/// uniqueness, and label interning.
fn build_flat_matcher(groups: &[IpGroup]) -> Result<IpRadixMatcher, String> {
    build_grouped_matcher(groups)
}

// ===== Typed overlay (all-Optional) =====

/// All-Optional typed overlay for `GlobalConnectionIpRestriction`.
///
/// Referenced via `overrideRef` from a `GlobalConnectionIpRestrictionEntry` in
/// `EdgionStreamPlugins`. Only fields that are `Some` replace the corresponding
/// field in the base config; `None` fields keep the base value. Collections
/// (`profiles`) are replaced **whole** — never element-wise merged.
///
/// # Security invariant
/// The `base` field in `GlobalConnectionIpRestrictionEntry` MUST be authored as
/// the conservative/safe default (e.g. `defaultAction: Deny` with an explicit
/// allowlist). Fail-to-base means any overlay error falls back to the base — if
/// the base is permissive (`defaultAction: Allow` with no deny list), fail-to-base
/// silently permits all connections. See design §3/§4.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalConnIpRestrictionOverride {
    /// Override the kill-switch. When `Some(false)`, the merged effective config
    /// has `enable=false`, bypassing all IP checks (allow all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,

    /// Override the active profile selector. Must exist in the merged `profiles`
    /// (either base or, if `profiles` is also set, the override profiles).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,

    /// Whole-replace the profiles map. When `Some`, this completely substitutes
    /// the base profiles — no element-wise merging. When `None`, base profiles
    /// are kept unchanged.
    ///
    /// Note: deserialized `ProfileRules` entries do NOT carry compiled matchers
    /// (`allow_matcher`/`deny_matcher` are `#[serde(skip)]`). The gateway
    /// calls `validate_and_init` on the merged effective config to compile them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiles: Option<HashMap<String, ProfileRules>>,
}

impl GlobalConnIpRestrictionOverride {
    /// Standalone validation — checks only the fields that are present.
    ///
    /// Called by the controller at EdgionData ingest time (via `validate_pre_parse`)
    /// and optionally by the gateway overlay-path before validation of the full
    /// merged effective config.
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        if let Some(profiles) = &mut self.profiles {
            if profiles.is_empty() {
                return Err("GlobalConnIpRestrictionOverride: profiles must not be empty when specified".to_string());
            }
            // If active_profile is also overridden, verify it exists in the overlay profiles.
            if let Some(active) = &self.active_profile {
                if !profiles.contains_key(active.as_str()) {
                    return Err(format!(
                        "GlobalConnIpRestrictionOverride: activeProfile '{}' not found in override profiles {:?}",
                        active,
                        profiles.keys().collect::<Vec<_>>()
                    ));
                }
            }
            // Pre-compile matchers for each profile in the overlay.
            for (name, profile) in profiles.iter_mut() {
                profile
                    .validate_and_init()
                    .map_err(|e| format!("profile '{}': {}", name, e))?;
            }
        }
        Ok(())
    }
}

/// Merge a `GlobalConnIpRestrictionOverride` onto a base `GlobalConnectionIpRestrictionData`.
///
/// Field semantics (strict replace, no deep-merge):
/// - Each `Some(v)` in the overlay replaces the corresponding base field.
/// - `None` keeps the base field unchanged.
/// - `profiles`: whole-replace when `Some` (not element-wise merged).
///
/// The returned `GlobalConnectionIpRestrictionData` has compiled matchers only
/// for profiles whose data came from the **base** (via `Clone`). Profiles injected
/// by the overlay do NOT have compiled matchers. The caller must call
/// `validate_and_init` on the result before using it for IP checks.
pub fn merge_gcir_override(
    mut base: GlobalConnectionIpRestrictionData,
    overlay: &GlobalConnIpRestrictionOverride,
) -> GlobalConnectionIpRestrictionData {
    if let Some(enable) = overlay.enable {
        base.enable = enable;
    }
    if let Some(ref active_profile) = overlay.active_profile {
        base.active_profile = active_profile.clone();
    }
    if let Some(ref profiles) = overlay.profiles {
        // Whole-replace: new profiles map completely substitutes the base map.
        // Cloned entries from deserialization have no compiled matchers.
        base.profiles = profiles.clone();
    }
    // description is not overridable
    base
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
                allow_matcher: None,
                deny_matcher: None,
            },
        )
    }

    #[test]
    fn construction_and_serde_roundtrip() {
        let (k, v) = valid_profile("p1");
        let data = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([(k, v)]),
            description: Some("test".to_string()),
        };
        let j = serde_json::to_value(&data).unwrap();
        let back: GlobalConnectionIpRestrictionData = serde_json::from_value(j).unwrap();
        assert_eq!(back.active_profile, "p1");
        assert!(back.enable);
    }

    fn empty_profile() -> ProfileRules {
        ProfileRules {
            allow: None,
            deny: None,
            default_action: DefaultAction::Allow,
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
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::new(),
            description: None,
        };
        let err = d.validate_and_init().unwrap_err();
        assert!(err.contains("at least one profile"), "{}", err);
    }

    #[test]
    fn validate_active_profile_must_exist_errs() {
        let (_, p) = valid_profile("p1");
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "nope".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
        };
        let err = d.validate_and_init().unwrap_err();
        assert!(err.contains("activeProfile") && err.contains("nope"), "{}", err);
    }

    #[test]
    fn validate_profile_requires_allow_or_deny() {
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), empty_profile())]),
            description: None,
        };
        let err = d.validate_and_init().unwrap_err();
        assert!(err.contains("at least one of") && err.contains("allow"), "{}", err);
    }

    #[test]
    fn validate_duplicate_group_names_errs() {
        let p = ProfileRules {
            allow: Some(vec![
                group("office", &["192.168.1.0/24"]),
                group("office", &["10.0.0.0/8"]),
            ]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
        };
        let err = d.validate_and_init().unwrap_err();
        assert!(err.contains("duplicate") && err.contains("office"), "{}", err);
    }

    #[test]
    fn validate_empty_cidrs_errs() {
        let p = ProfileRules {
            allow: Some(vec![group("office", &[])]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
        };
        let err = d.validate_and_init().unwrap_err();
        // Error message comes from validate_groups_structure: "group 'office' has no cidrs"
        assert!(err.contains("cidrs") && err.contains("no"), "{}", err);
    }

    #[test]
    fn validate_bad_cidr_errs() {
        let p = ProfileRules {
            allow: Some(vec![group("office", &["not-an-ip"])]),
            deny: None,
            default_action: DefaultAction::Deny,
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
        };
        assert!(d.validate_and_init().is_err());
    }

    #[test]
    fn validate_ok_builds_matchers() {
        let (k, v) = valid_profile("p1");
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([(k, v)]),
            description: None,
        };
        d.validate_and_init().unwrap();
        let p = d.profiles.get("p1").unwrap();
        assert!(p.allow_matcher.is_some());
    }

    fn init(data: &mut GlobalConnectionIpRestrictionData) {
        data.validate_and_init().unwrap();
    }

    /// Convenience: `check_ip` returns an `AccessDecision`; the legacy
    /// tests were boolean. This helper preserves their semantic intent.
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
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
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
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
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
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
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
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
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
            allow_matcher: None,
            deny_matcher: None,
        };
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([("p1".to_string(), p)]),
            description: None,
        };
        let err = d.validate_and_init().unwrap_err();
        // Should error cleanly mentioning empty group list (not opaque "Failed to build IpRadixMatcher")
        assert!(err.contains("empty"), "error should mention empty: {}", err);
    }

    // ===== GlobalConnIpRestrictionOverride tests =====

    fn make_base() -> GlobalConnectionIpRestrictionData {
        let (k, v) = valid_profile("p1");
        let mut d = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([(k, v)]),
            description: Some("base".to_string()),
        };
        d.validate_and_init().unwrap();
        d
    }

    #[test]
    fn override_all_none_is_identity() {
        let base = make_base();
        let ov = GlobalConnIpRestrictionOverride {
            enable: None,
            active_profile: None,
            profiles: None,
        };
        let merged = merge_gcir_override(base.clone(), &ov);
        assert_eq!(merged.enable, base.enable);
        assert_eq!(merged.active_profile, base.active_profile);
        // profiles cloned from base — should have same keys
        assert_eq!(merged.profiles.keys().collect::<Vec<_>>().len(), 1);
    }

    #[test]
    fn override_enable_false_disables() {
        let base = make_base();
        let ov = GlobalConnIpRestrictionOverride {
            enable: Some(false),
            active_profile: None,
            profiles: None,
        };
        let merged = merge_gcir_override(base, &ov);
        assert!(!merged.enable);
        // active_profile and profiles unchanged
        assert_eq!(merged.active_profile, "p1");
    }

    #[test]
    fn override_active_profile_switches_without_changing_profiles() {
        let (k2, v2) = valid_profile("p2");
        let mut base = make_base();
        base.profiles.insert(k2, v2);
        base.validate_and_init().unwrap();

        let ov = GlobalConnIpRestrictionOverride {
            enable: None,
            active_profile: Some("p2".to_string()),
            profiles: None,
        };
        let mut merged = merge_gcir_override(base.clone(), &ov);
        assert_eq!(merged.active_profile, "p2");
        // Both profiles still present (whole-replace was not triggered)
        assert!(merged.profiles.contains_key("p1"));
        assert!(merged.profiles.contains_key("p2"));
        // validate_and_init succeeds (p2 exists)
        assert!(merged.validate_and_init().is_ok());
    }

    #[test]
    fn override_profiles_whole_replace() {
        let base = make_base(); // has profile "p1"
        let (k_new, v_new) = valid_profile("new-profile");
        let ov = GlobalConnIpRestrictionOverride {
            enable: None,
            active_profile: Some("new-profile".to_string()),
            profiles: Some(HashMap::from([(k_new, v_new)])),
        };
        let mut merged = merge_gcir_override(base, &ov);
        // p1 is GONE (whole-replace), new-profile is present
        assert!(!merged.profiles.contains_key("p1"));
        assert!(merged.profiles.contains_key("new-profile"));
        assert_eq!(merged.active_profile, "new-profile");
        assert!(merged.validate_and_init().is_ok());
    }

    #[test]
    fn override_validate_empty_profiles_errs() {
        let mut ov = GlobalConnIpRestrictionOverride {
            enable: None,
            active_profile: None,
            profiles: Some(HashMap::new()),
        };
        let err = ov.validate_and_init().unwrap_err();
        assert!(err.contains("empty"), "{}", err);
    }

    #[test]
    fn override_validate_active_profile_missing_in_override_profiles_errs() {
        let (k, v) = valid_profile("p1");
        let mut ov = GlobalConnIpRestrictionOverride {
            enable: None,
            active_profile: Some("nonexistent".to_string()),
            profiles: Some(HashMap::from([(k, v)])),
        };
        let err = ov.validate_and_init().unwrap_err();
        assert!(err.contains("nonexistent"), "{}", err);
    }

    #[test]
    fn override_validate_ok_when_profiles_none() {
        let mut ov = GlobalConnIpRestrictionOverride {
            enable: Some(false),
            active_profile: None,
            profiles: None,
        };
        // No profiles to validate — should succeed
        assert!(ov.validate_and_init().is_ok());
    }

    #[test]
    fn merged_result_requires_validate_and_init_when_profiles_replaced() {
        let base = make_base();
        let (k_new, v_new) = valid_profile("new");
        let ov = GlobalConnIpRestrictionOverride {
            enable: None,
            active_profile: Some("new".to_string()),
            profiles: Some(HashMap::from([(k_new.clone(), v_new)])),
        };
        let mut merged = merge_gcir_override(base, &ov);
        // Matchers NOT compiled on the overlay profiles (serde::skip, so they are None)
        let raw_profile = merged.profiles.get("new").unwrap();
        assert!(
            raw_profile.allow_matcher.is_none(),
            "matchers absent before validate_and_init"
        );

        // After validate_and_init, matchers are built
        merged.validate_and_init().unwrap();
        let compiled_profile = merged.profiles.get("new").unwrap();
        assert!(
            compiled_profile.allow_matcher.is_some(),
            "matchers must be compiled after validate_and_init"
        );
    }
}

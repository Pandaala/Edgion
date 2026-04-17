//! GlobalConnectionIpRestriction PluginMetaData variant — L4 TCP connection-phase IP restriction.

use crate::core::common::matcher::ip_cidr_helpers::build_radix_matcher;
use crate::core::common::matcher::ip_radix_tree::IpRadixMatcher;
use crate::types::resources::edgion_plugins::DefaultAction;
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
    pub(crate) allow_matcher: Option<IpRadixMatcher>,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) deny_matcher: Option<IpRadixMatcher>,
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

        if let Some(allow) = &self.allow {
            self.allow_matcher = Some(build_flat_matcher(allow)?);
        }
        if let Some(deny) = &self.deny {
            self.deny_matcher = Some(build_flat_matcher(deny)?);
        }
        Ok(())
    }

    /// Check whether an IP should be allowed under this profile's rules.
    /// Priority: deny > allow > default_action.
    ///
    /// `match_ip().is_some()` means "IP is covered by some CIDR in the matcher".
    /// build_flat_matcher always inserts with a sentinel `true` value; the stored
    /// value is not semantically meaningful (we only care if any CIDR matched).
    pub fn check_ip(&self, ip: &IpAddr) -> bool {
        if let Some(deny) = &self.deny_matcher {
            if deny.match_ip(ip).is_some() {
                return false;
            }
        }
        if let Some(allow) = &self.allow_matcher {
            return allow.match_ip(ip).is_some();
        }
        matches!(self.default_action, DefaultAction::Allow)
    }
}

/// Flatten `Vec<IpGroup>` into a single IpRadixMatcher, while enforcing uniqueness of group names.
fn build_flat_matcher(groups: &[IpGroup]) -> Result<IpRadixMatcher, String> {
    if groups.is_empty() {
        return Err("group list is empty".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    let mut all_cidrs: Vec<String> = Vec::new();
    for g in groups {
        if !seen.insert(g.name.as_str()) {
            return Err(format!("duplicate group name '{}'", g.name));
        }
        if g.cidrs.is_empty() {
            return Err(format!("group '{}' cidrs is empty", g.name));
        }
        all_cidrs.extend(g.cidrs.iter().cloned());
    }
    build_radix_matcher(&all_cidrs)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(err.contains("cidrs") && err.contains("empty"), "{}", err);
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
        assert!(p.check_ip(&"192.168.1.50".parse().unwrap()));
        assert!(!p.check_ip(&"8.8.8.8".parse().unwrap()));
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
        assert!(p.check_ip(&"10.0.0.1".parse().unwrap()));
        assert!(!p.check_ip(&"10.0.0.100".parse().unwrap()));
        assert!(!p.check_ip(&"8.8.8.8".parse().unwrap()));
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
        assert!(!p.check_ip(&"1.2.3.4".parse().unwrap()));
        assert!(p.check_ip(&"8.8.8.8".parse().unwrap()));
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
        assert!(p.check_ip(&"173.245.48.1".parse().unwrap()));
        assert!(p.check_ip(&"192.168.1.50".parse().unwrap()));
        assert!(!p.check_ip(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn validate_allow_some_empty_errs_cleanly() {
        let p = ProfileRules {
            allow: Some(vec![]),  // Some but empty
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
}

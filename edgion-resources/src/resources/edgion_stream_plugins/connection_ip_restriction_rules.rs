//! L4-specific inline IP restriction rules used by `EdgionStreamPlugin::IpRestriction`.
//!
//! Contrast with `IpRestrictionConfig` (HTTP-layer): this omits `ip_source`, `message`,
//! and `status` because L4 has no headers or protocol-level response (rejection = silent close).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::matcher::ip_cidr_helpers::{build_grouped_matcher_partial, evaluate_ip_access, validate_refs_structure};
use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_config_data::IpGroup;
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction, PluginValidation};

/// Legacy HTTP-layer field names that must be flagged when found on L4 configs.
///
/// These belong to the old HTTP `IpRestrictionConfig` schema and have no meaning in L4
/// configs. Detecting them at preparse-time lets the Controller surface a
/// `LegacySchemaDeprecated` status condition instead of failing CR deserialization at the
/// watcher level (which would silently bypass L4 enforcement and give operators no
/// migration hint).
const LEGACY_HTTP_FIELDS: &[&str] = &["ipSource", "message", "status"];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionIpRestrictionRules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<IpGroup>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<IpGroup>>,

    #[serde(default)]
    pub default_action: DefaultAction,

    /// Allow-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_refs: Option<Vec<crate::resources::edgion_config_data::EdgionConfigDataRef>>,

    /// Deny-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_refs: Option<Vec<crate::resources::edgion_config_data::EdgionConfigDataRef>>,

    /// Catch-all for any field not recognized by the struct. Populated by serde during
    /// deserialization so the Controller's preparse can identify legacy HTTP fields
    /// (`ipSource`/`message`/`status`) from the old schema and emit a migration status
    /// condition. Never written back to K8s (Controller only patches `/status`).
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(skip)]
    unknown_fields: BTreeMap<String, serde_json::Value>,

    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) allow_matcher: Option<IpRadixMatcher>,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) deny_matcher: Option<IpRadixMatcher>,
}

impl Default for ConnectionIpRestrictionRules {
    fn default() -> Self {
        Self {
            allow: None,
            deny: None,
            default_action: DefaultAction::default(),
            allow_refs: None,
            deny_refs: None,
            unknown_fields: BTreeMap::new(),
            allow_matcher: None,
            deny_matcher: None,
        }
    }
}

impl ConnectionIpRestrictionRules {
    /// Return any legacy HTTP-layer field names (`ipSource`/`message`/`status`) captured
    /// during deserialization. A non-empty result means the YAML uses the old HTTP schema
    /// and the operator must migrate.
    pub fn legacy_http_fields(&self) -> Vec<&'static str> {
        LEGACY_HTTP_FIELDS
            .iter()
            .filter(|name| self.unknown_fields.contains_key(**name))
            .copied()
            .collect()
    }

    pub fn validate_and_init(&mut self) -> Result<(), String> {
        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Err("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }
        if let Some(groups) = &self.allow {
            if !groups.is_empty() {
                let (m, _errs) = build_grouped_matcher_partial(groups);
                self.allow_matcher = Some(m);
            }
        }
        if let Some(groups) = &self.deny {
            if !groups.is_empty() {
                let (m, _errs) = build_grouped_matcher_partial(groups);
                self.deny_matcher = Some(m);
            }
        }
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

/// Stateless structural validation mirroring HTTP `IpRestrictionConfig`. The
/// controller preparse calls this instead of the side-effectful
/// `validate_and_init` (which builds radix matchers and is reserved for the
/// data-plane builders in `StreamIpRestriction::new`).
impl PluginValidation for ConnectionIpRestrictionRules {
    fn validation_error(&self) -> Option<String> {
        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }
        if let Some(refs) = &self.allow_refs {
            if let Err(e) = validate_refs_structure(refs) {
                return Some(format!("allowRefs: {}", e));
            }
        }
        if let Some(refs) = &self.deny_refs {
            if let Err(e) = validate_refs_structure(refs) {
                return Some(format!("denyRefs: {}", e));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_mode_true_when_only_allow_refs_set() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let r = ConnectionIpRestrictionRules {
            allow_refs: Some(vec![EdgionConfigDataRef {
                name: "corp".into(),
                namespace: None,
            }]),
            ..Default::default()
        };
        assert!(r.allow_mode());
    }

    #[test]
    fn refs_only_config_passes_validation() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let mut r = ConnectionIpRestrictionRules {
            deny_refs: Some(vec![EdgionConfigDataRef {
                name: "blocklist".into(),
                namespace: None,
            }]),
            ..Default::default()
        };
        assert!(r.validate_and_init().is_ok());
        assert_eq!(r.validation_error(), None);
    }

    #[test]
    fn with_refs_deny_union_blocks_referenced_ip() {
        use crate::matcher::ip_cidr_helpers::build_grouped_matcher;
        let mut r = ConnectionIpRestrictionRules {
            deny: Some(vec![group("inline", &["1.1.1.1"])]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        let ref_deny = build_grouped_matcher(&[group("reflist", &["2.2.2.2"])]).unwrap();
        assert!(matches!(
            r.check_ip_with_refs(&"2.2.2.2".parse().unwrap(), &[&ref_deny], &[]),
            AccessDecision::DenyByGroup(_)
        ));
        assert!(is_allowed(r.check_ip_with_refs(
            &"8.8.8.8".parse().unwrap(),
            &[&ref_deny],
            &[]
        )));
    }

    #[test]
    fn allow_refs_missing_denies_all_in_allow_mode() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let r = ConnectionIpRestrictionRules {
            allow_refs: Some(vec![EdgionConfigDataRef {
                name: "missing".into(),
                namespace: None,
            }]),
            ..Default::default()
        };
        assert!(matches!(
            r.check_ip_with_refs(&"8.8.8.8".parse().unwrap(), &[], &[]),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    fn group(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.to_string(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Maps an `AccessDecision` to a boolean allow/deny for test assertions.
    fn is_allowed(decision: AccessDecision) -> bool {
        matches!(
            decision,
            AccessDecision::AllowByGroup(_) | AccessDecision::Default(DefaultAction::Allow)
        )
    }

    #[test]
    fn must_have_allow_or_deny() {
        let mut r = ConnectionIpRestrictionRules::default();
        assert!(r.validate_and_init().is_err());
    }

    #[test]
    fn allow_whitelist_ok() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec![group("office", &["192.168.1.0/24"])]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        assert!(is_allowed(r.check_ip(&"192.168.1.50".parse().unwrap())));
        assert!(matches!(
            r.check_ip(&"8.8.8.8".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn deny_wins_over_allow() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec![group("internal", &["10.0.0.0/8"])]),
            deny: Some(vec![group("blocked", &["10.0.0.100"])]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        assert!(is_allowed(r.check_ip(&"10.0.0.1".parse().unwrap())));
        match r.check_ip(&"10.0.0.100".parse().unwrap()) {
            AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "blocked"),
            other => panic!("expected DenyByGroup, got {:?}", other),
        }
    }

    #[test]
    fn legacy_http_fields_captured_for_preparse() {
        // Old HTTP-schema fields must round-trip into `unknown_fields` so the Controller's
        // preparse step can emit a `LegacySchemaDeprecated` condition rather than failing
        // CR deserialization at the watcher level.
        let json = r#"{
            "allow": [{"name":"office","cidrs":["192.168.1.0/24"]}],
            "ipSource": "RemoteAddr",
            "message": "blocked",
            "status": 403
        }"#;
        let rules: ConnectionIpRestrictionRules =
            serde_json::from_str(json).expect("legacy fields should now parse (detection moved to preparse)");
        let mut legacy = rules.legacy_http_fields();
        legacy.sort();
        assert_eq!(legacy, vec!["ipSource", "message", "status"]);
    }

    #[test]
    fn clean_config_has_no_unknown_fields() {
        let json = r#"{
            "allow": [{"name":"office","cidrs":["192.168.1.0/24"]}],
            "defaultAction": "deny"
        }"#;
        let rules: ConnectionIpRestrictionRules = serde_json::from_str(json).unwrap();
        assert!(rules.legacy_http_fields().is_empty());
        assert!(rules.unknown_fields.is_empty());
    }

    #[test]
    fn non_legacy_unknown_field_does_not_trigger_legacy_detection() {
        // A stray (non-legacy) field must not masquerade as a legacy-schema violation;
        // preparse only cares about the three HTTP fields from the old schema.
        let json = r#"{
            "allow": [{"name":"office","cidrs":["192.168.1.0/24"]}],
            "typoedField": "oops"
        }"#;
        let rules: ConnectionIpRestrictionRules = serde_json::from_str(json).unwrap();
        assert!(rules.legacy_http_fields().is_empty());
        assert!(rules.unknown_fields.contains_key("typoedField"));
    }

    #[test]
    fn deny_only_default_deny_blocks_all_unmatched() {
        let mut r = ConnectionIpRestrictionRules {
            deny: Some(vec![group("blocked", &["1.2.3.4"])]),
            default_action: DefaultAction::Deny,
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        // In deny list -> DenyByGroup.
        assert!(matches!(
            r.check_ip(&"1.2.3.4".parse().unwrap()),
            AccessDecision::DenyByGroup(_)
        ));
        // Not in deny, default Deny -> Default(Deny).
        assert!(matches!(
            r.check_ip(&"8.8.8.8".parse().unwrap()),
            AccessDecision::Default(DefaultAction::Deny)
        ));
    }

    #[test]
    fn ipv6_allow_and_deny_work() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec![group("v6-net", &["2001:db8::/32"])]),
            deny: Some(vec![group("v6-bad", &["2001:db8:dead::/48"])]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        assert!(is_allowed(r.check_ip(&"2001:db8::1".parse().unwrap()))); // in allow
        assert!(matches!(
            r.check_ip(&"2001:db8:dead::1".parse().unwrap()),
            AccessDecision::DenyByGroup(_)
        )); // in deny (wins)
        assert!(matches!(
            r.check_ip(&"2001:db9::1".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        )); // outside allow
    }

    #[test]
    fn whitelist_miss_returns_specific_variant() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec![group("office", &["192.168.0.0/16"])]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        let outside: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(matches!(r.check_ip(&outside), AccessDecision::DenyByWhitelistMiss));
    }

    #[test]
    fn validation_error_both_none_reports_missing_lists() {
        let rules = ConnectionIpRestrictionRules::default();
        assert_eq!(
            rules.validation_error(),
            Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
        );
    }

    #[test]
    fn validation_error_is_stateless_and_keeps_matchers_unset() {
        let rules = ConnectionIpRestrictionRules {
            allow: Some(vec![group("office", &["192.168.1.0/24"])]),
            ..Default::default()
        };
        assert!(rules.validation_error().is_none());
        // Stateless: must not have built matchers as a side effect.
        assert!(rules.allow_matcher.is_none());
        assert!(rules.deny_matcher.is_none());
    }

    #[test]
    fn validation_error_tolerates_invalid_cidr() {
        // Partial-good (CIDR-only leniency): a bad inline CIDR no longer rejects at
        // admission. Structural (non-CIDR) errors are still surfaced elsewhere.
        let rules = ConnectionIpRestrictionRules {
            allow: Some(vec![group("bad", &["not-an-ip"])]),
            ..Default::default()
        };
        assert!(rules.validation_error().is_none());
    }

    #[test]
    fn deny_priority_returns_deny_group() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec![group("allow_all", &["10.0.0.0/8"])]),
            deny: Some(vec![group("blocked", &["10.0.0.100/32"])]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        let blocked: IpAddr = "10.0.0.100".parse().unwrap();
        match r.check_ip(&blocked) {
            AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "blocked"),
            other => panic!("expected DenyByGroup, got {:?}", other),
        }
    }
}

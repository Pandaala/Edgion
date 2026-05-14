//! L4-specific inline IP restriction rules used by `EdgionStreamPlugin::IpRestriction`.
//!
//! Contrast with `IpRestrictionConfig` (HTTP-layer): this omits `ip_source`, `message`,
//! and `status` because L4 has no headers or protocol-level response (rejection = silent close).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::core::common::matcher::ip_cidr_helpers::build_grouped_matcher;
use crate::core::common::matcher::ip_radix_tree::IpRadixMatcher;
use crate::types::resources::edgion_plugins::{AccessDecision, DefaultAction};
use crate::types::resources::plugin_metadata::IpGroup;

/// Legacy HTTP-layer field names that must be flagged when found on L4 configs.
///
/// These belonged to the pre-`ec4e000` HTTP `IpRestrictionConfig` schema. Detecting them
/// at preparse-time lets the Controller surface a `LegacySchemaDeprecated` status condition
/// instead of failing the whole CR deserialization at the watcher level (which would silently
/// bypass the L4 enforcement and give operators no migration hint).
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
            unknown_fields: BTreeMap::new(),
            allow_matcher: None,
            deny_matcher: None,
        }
    }
}

impl ConnectionIpRestrictionRules {
    /// Return any legacy HTTP-layer field names (`ipSource`/`message`/`status`) captured
    /// during deserialization. Non-empty result means the YAML predates the `ec4e000`
    /// schema reshape and the operator must migrate.
    pub fn legacy_http_fields(&self) -> Vec<&'static str> {
        LEGACY_HTTP_FIELDS
            .iter()
            .filter(|name| self.unknown_fields.contains_key(**name))
            .copied()
            .collect()
    }

    pub fn validate_and_init(&mut self) -> Result<(), String> {
        let allow_nonempty = self.allow.as_ref().is_some_and(|v| !v.is_empty());
        let deny_nonempty = self.deny.as_ref().is_some_and(|v| !v.is_empty());
        if !allow_nonempty && !deny_nonempty {
            return Err("At least one of 'allow' or 'deny' must be non-empty".to_string());
        }
        if let Some(groups) = &self.allow {
            if !groups.is_empty() {
                self.allow_matcher = Some(build_grouped_matcher(groups)?);
            }
        }
        if let Some(groups) = &self.deny {
            if !groups.is_empty() {
                self.deny_matcher = Some(build_grouped_matcher(groups)?);
            }
        }
        Ok(())
    }

    /// Returns the full access decision (group name included).
    ///
    /// Priority: deny > allow > default_action. See spec §4.6.
    pub fn check_ip(&self, ip: &IpAddr) -> AccessDecision {
        if let Some(deny) = &self.deny_matcher {
            if let Some(g) = deny.matched_group_arc(ip) {
                return AccessDecision::DenyByGroup(g);
            }
        }
        if let Some(allow) = &self.allow_matcher {
            if let Some(g) = allow.matched_group_arc(ip) {
                return AccessDecision::AllowByGroup(g);
            }
            return AccessDecision::DenyByWhitelistMiss;
        }
        AccessDecision::Default(self.default_action.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.to_string(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Convenience: `check_ip` returns an `AccessDecision`; the legacy
    /// tests were boolean. This helper preserves their semantic intent.
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
        // Old schema (pre-ec4e000) HTTP-layer fields must round-trip into `unknown_fields`
        // so the Controller's preparse step can emit a `LegacySchemaDeprecated` condition
        // rather than failing the whole CR decode upstream at the watcher.
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
        // preparse only cares about the three HTTP fields from the pre-ec4e000 schema.
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

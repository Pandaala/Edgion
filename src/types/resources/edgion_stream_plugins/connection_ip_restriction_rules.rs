//! L4-specific inline IP restriction rules used by `EdgionStreamPlugin::IpRestriction`.
//!
//! Contrast with `IpRestrictionConfig` (HTTP-layer): this omits `ip_source`, `message`,
//! and `status` because L4 has no headers or protocol-level response (rejection = silent close).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::core::common::matcher::ip_cidr_helpers::build_radix_matcher;
use crate::core::common::matcher::ip_radix_tree::IpRadixMatcher;
use crate::types::resources::edgion_plugins::DefaultAction;

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
    pub allow: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,

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
        if self.allow.is_none() && self.deny.is_none() {
            return Err("At least one of 'allow' or 'deny' must be specified".to_string());
        }
        if let Some(allow) = &self.allow {
            if allow.is_empty() {
                return Err("'allow' list cannot be empty".to_string());
            }
            self.allow_matcher = Some(build_radix_matcher(allow)?);
        }
        if let Some(deny) = &self.deny {
            if deny.is_empty() {
                return Err("'deny' list cannot be empty".to_string());
            }
            self.deny_matcher = Some(build_radix_matcher(deny)?);
        }
        Ok(())
    }

    /// Priority: deny > allow > default_action.
    ///
    /// `match_ip().is_some()` means "IP is covered by some CIDR in the matcher".
    /// build_radix_matcher inserts all CIDRs with a sentinel `true` value; the
    /// stored value is not semantically meaningful (we only care if any CIDR matched).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn must_have_allow_or_deny() {
        let mut r = ConnectionIpRestrictionRules::default();
        assert!(r.validate_and_init().is_err());
    }

    #[test]
    fn allow_whitelist_ok() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec!["192.168.1.0/24".to_string()]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        assert!(r.check_ip(&"192.168.1.50".parse().unwrap()));
        assert!(!r.check_ip(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn deny_wins_over_allow() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec!["10.0.0.0/8".to_string()]),
            deny: Some(vec!["10.0.0.100".to_string()]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        assert!(r.check_ip(&"10.0.0.1".parse().unwrap()));
        assert!(!r.check_ip(&"10.0.0.100".parse().unwrap()));
    }

    #[test]
    fn legacy_http_fields_captured_for_preparse() {
        // Old schema (pre-ec4e000) HTTP-layer fields must round-trip into `unknown_fields`
        // so the Controller's preparse step can emit a `LegacySchemaDeprecated` condition
        // rather than failing the whole CR decode upstream at the watcher.
        let json = r#"{
            "allow": ["192.168.1.0/24"],
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
            "allow": ["192.168.1.0/24"],
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
            "allow": ["192.168.1.0/24"],
            "typoedField": "oops"
        }"#;
        let rules: ConnectionIpRestrictionRules = serde_json::from_str(json).unwrap();
        assert!(rules.legacy_http_fields().is_empty());
        assert!(rules.unknown_fields.contains_key("typoedField"));
    }

    #[test]
    fn deny_only_default_deny_blocks_all_unmatched() {
        let mut r = ConnectionIpRestrictionRules {
            deny: Some(vec!["1.2.3.4".to_string()]),
            default_action: DefaultAction::Deny,
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        assert!(!r.check_ip(&"1.2.3.4".parse().unwrap())); // in deny list
        assert!(!r.check_ip(&"8.8.8.8".parse().unwrap())); // not in deny, default Deny
    }

    #[test]
    fn ipv6_allow_and_deny_work() {
        let mut r = ConnectionIpRestrictionRules {
            allow: Some(vec!["2001:db8::/32".to_string()]),
            deny: Some(vec!["2001:db8:dead::/48".to_string()]),
            ..Default::default()
        };
        r.validate_and_init().unwrap();
        assert!(r.check_ip(&"2001:db8::1".parse().unwrap())); // in allow
        assert!(!r.check_ip(&"2001:db8:dead::1".parse().unwrap())); // in deny (wins)
        assert!(!r.check_ip(&"2001:db9::1".parse().unwrap())); // outside allow
    }
}

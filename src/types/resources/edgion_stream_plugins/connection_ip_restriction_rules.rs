//! L4-specific inline IP restriction rules used by `EdgionStreamPlugin::IpRestriction`.
//!
//! Contrast with `IpRestrictionConfig` (HTTP-layer): this omits `ip_source`, `message`,
//! and `status` because L4 has no headers or protocol-level response (rejection = silent close).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use crate::core::common::matcher::ip_cidr_helpers::build_radix_matcher;
use crate::core::common::matcher::ip_radix_tree::IpRadixMatcher;
use crate::types::resources::edgion_plugins::DefaultAction;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionIpRestrictionRules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,

    #[serde(default)]
    pub default_action: DefaultAction,

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
            allow_matcher: None,
            deny_matcher: None,
        }
    }
}

impl ConnectionIpRestrictionRules {
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
    fn unknown_fields_rejected() {
        // deny_unknown_fields prevents silent drops of ip_source / message / status from old configs.
        let json = r#"{
            "allow": ["192.168.1.0/24"],
            "message": "blocked"
        }"#;
        let result: Result<ConnectionIpRestrictionRules, _> = serde_json::from_str(json);
        assert!(result.is_err(), "should reject unknown 'message' field");
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

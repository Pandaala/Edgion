//! Access decision returned by IP-list matchers.
//!
//! Used by IpRestriction, ConnectionIpRestrictionRules, and
//! GlobalConnectionIpRestriction::ProfileRules to communicate
//! the matched group (or absence of match) back to the caller,
//! so observability layers can name the group that triggered the
//! allow/deny outcome.

use std::sync::Arc;

use super::DefaultAction;

/// Outcome of an IP access check against a configured allow/deny pair.
#[derive(Debug, Clone)]
pub enum AccessDecision {
    /// IP matched an entry in the allow list. Carries the group name.
    AllowByGroup(Arc<str>),

    /// IP matched an entry in the deny list (deny precedes allow).
    /// Carries the group name.
    DenyByGroup(Arc<str>),

    /// Allow list is configured (whitelist mode) but the IP did not match
    /// any allow entry, and the deny list did not match either (or is
    /// absent). The rejection reason is "not in any allow group".
    DenyByWhitelistMiss,

    /// No allow or deny rule matched. The caller applies the configured
    /// default action.
    Default(DefaultAction),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_by_group_carries_name() {
        let d = AccessDecision::AllowByGroup(Arc::from("office"));
        match d {
            AccessDecision::AllowByGroup(g) => assert_eq!(&*g, "office"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deny_by_whitelist_miss_no_payload() {
        let d = AccessDecision::DenyByWhitelistMiss;
        assert!(matches!(d, AccessDecision::DenyByWhitelistMiss));
    }

    #[test]
    fn default_carries_action() {
        let d = AccessDecision::Default(DefaultAction::Deny);
        assert!(matches!(d, AccessDecision::Default(DefaultAction::Deny)));
    }

    #[test]
    fn clone_works() {
        let d = AccessDecision::DenyByGroup(Arc::from("blocklist"));
        let c = d.clone();
        match (d, c) {
            (AccessDecision::DenyByGroup(a), AccessDecision::DenyByGroup(b)) => {
                assert_eq!(a, b);
                assert!(Arc::ptr_eq(&a, &b));
            }
            _ => panic!("clone changed variant"),
        }
    }
}

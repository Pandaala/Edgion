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
#[path = "../../../tests/unit/resources/edgion_plugins/access_decision_test.rs"]
mod tests;

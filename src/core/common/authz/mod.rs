//! Generic authorization primitives shared by the Admin API.
//!
//! `Authorizer` decides `(role, verb, kind) -> Decision`. The `Center` role's
//! policy is held behind an `ArcSwap` so it can be hot-swapped on reload without
//! restarting the federation client. Default-deny: an empty policy authorizes
//! nothing. Pure, no I/O.

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::core::common::config::FedRbacConfig;

/// The closed set of verbs the classifier can produce. Safe as a metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    List,
    ListKeys,
    Watch,
    Create,
    Update,
    Delete,
    Failover,
    AcmeTrigger,
    Reload,
    ServerInfo,
    Diagnostics,
    WipeAll,
    Debug,
}

impl Verb {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verb::Get => "get",
            Verb::List => "list",
            Verb::ListKeys => "list-keys",
            Verb::Watch => "watch",
            Verb::Create => "create",
            Verb::Update => "update",
            Verb::Delete => "delete",
            Verb::Failover => "failover",
            Verb::AcmeTrigger => "acme-trigger",
            Verb::Reload => "reload",
            Verb::ServerInfo => "server-info",
            Verb::Diagnostics => "diagnostics",
            Verb::WipeAll => "wipe-all",
            Verb::Debug => "debug",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Verb> {
        Some(match s {
            "get" => Verb::Get,
            "list" => Verb::List,
            "list-keys" => Verb::ListKeys,
            "watch" => Verb::Watch,
            "create" => Verb::Create,
            "update" => Verb::Update,
            "delete" => Verb::Delete,
            "failover" => Verb::Failover,
            "acme-trigger" => Verb::AcmeTrigger,
            "reload" => Verb::Reload,
            "server-info" => Verb::ServerInfo,
            "diagnostics" => Verb::Diagnostics,
            "wipe-all" => Verb::WipeAll,
            "debug" => Verb::Debug,
            _ => return None,
        })
    }
}

/// The identity assigned to a request.
#[derive(Debug, Clone)]
pub enum Role {
    /// Authenticated local/OIDC admin (and no-auth-mode local admin): allow-all.
    Superuser,
    /// The federation channel identity: evaluated against the policy.
    Center,
}

#[derive(Debug, Clone)]
struct CompiledRule {
    verbs: Vec<String>,
    kinds: Vec<String>,
}

/// Default-deny policy for the `Center` role.
#[derive(Debug, Clone, Default)]
pub struct CenterPolicy {
    rules: Vec<CompiledRule>,
}

impl CenterPolicy {
    pub fn deny_all() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn from_config(cfg: &FedRbacConfig) -> Self {
        let rules = cfg
            .allow
            .iter()
            .map(|r| CompiledRule {
                verbs: r.verbs.iter().map(|s| s.to_ascii_lowercase()).collect(),
                kinds: r.kinds.clone(),
            })
            .collect();
        Self { rules }
    }

    fn permits(&self, verb: Verb, kind: &str) -> bool {
        let v = verb.as_str();
        self.rules
            .iter()
            .any(|r| r.verbs.iter().any(|x| x == "*" || x == v) && r.kinds.iter().any(|x| x == "*" || x == kind))
    }
}

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    pub fn allowed(self) -> bool {
        self == Decision::Allow
    }
}

/// Shared authorizer holding a hot-swappable Center policy.
pub struct Authorizer {
    center_policy: ArcSwap<CenterPolicy>,
}

impl Authorizer {
    pub fn new(policy: CenterPolicy) -> Self {
        Self {
            center_policy: ArcSwap::from_pointee(policy),
        }
    }

    /// Hot-swap the Center policy (per-request snapshots pick this up).
    pub fn swap_policy(&self, policy: CenterPolicy) {
        self.center_policy.store(Arc::new(policy));
    }

    pub fn authorize(&self, role: &Role, verb: Verb, kind: &str) -> Decision {
        match role {
            Role::Superuser => Decision::Allow,
            Role::Center => {
                if self.center_policy.load().permits(verb, kind) {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::config::{FedAllowRule, FedRbacConfig};

    fn policy(rules: Vec<(&[&str], &[&str])>) -> CenterPolicy {
        CenterPolicy::from_config(&FedRbacConfig {
            allow: rules
                .into_iter()
                .map(|(v, k)| FedAllowRule {
                    verbs: v.iter().map(|s| s.to_string()).collect(),
                    kinds: k.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
        })
    }

    #[test]
    fn superuser_allows_everything() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(az.authorize(&Role::Superuser, Verb::Delete, "Secret").allowed());
    }

    #[test]
    fn empty_policy_denies_all() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(!az.authorize(&Role::Center, Verb::Get, "PluginMetaData").allowed());
    }

    #[test]
    fn center_policy_matches_exact_and_wildcard() {
        let az = Authorizer::new(policy(vec![
            (&["get", "list"], &["PluginMetaData"]),
            (&["reload"], &["*"]),
        ]));
        assert!(az.authorize(&Role::Center, Verb::Get, "PluginMetaData").allowed());
        assert!(!az.authorize(&Role::Center, Verb::Delete, "PluginMetaData").allowed());
        assert!(az.authorize(&Role::Center, Verb::Reload, "AnythingHere").allowed());
        assert!(!az.authorize(&Role::Center, Verb::Get, "Secret").allowed());
    }

    #[test]
    fn policy_hot_swaps() {
        let az = Authorizer::new(CenterPolicy::deny_all());
        assert!(!az.authorize(&Role::Center, Verb::Get, "PluginMetaData").allowed());
        az.swap_policy(policy(vec![(&["get"], &["PluginMetaData"])]));
        assert!(az.authorize(&Role::Center, Verb::Get, "PluginMetaData").allowed());
    }

    #[test]
    fn center_policy_verb_is_case_insensitive() {
        let az = Authorizer::new(policy(vec![(&["GET", "List"], &["PluginMetaData"])]));
        assert!(az.authorize(&Role::Center, Verb::Get, "PluginMetaData").allowed());
        assert!(az.authorize(&Role::Center, Verb::List, "PluginMetaData").allowed());
    }

    #[test]
    fn verb_roundtrips_string() {
        assert_eq!(Verb::Get.as_str(), "get");
        assert_eq!(Verb::from_str("failover"), Some(Verb::Failover));
        assert_eq!(Verb::from_str("nonsense"), None);
    }
}

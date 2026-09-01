//! Controller-side ownership index for inbound PROXY protocol listener policy.
//!
//! A physical listener may be declared by more than one Gateway. All
//! contributors must publish the same effective policy before the listener can
//! be programmed. Both directions live behind one lock so a Gateway update is
//! observed as one whole-resource replacement.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use edgion_resources::listener_key::ListenerKey;
use edgion_resources::resources::proxy_protocol::CanonicalProxyProtocolTrustPolicy;
use parking_lot::RwLock;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// Canonical declaration fingerprint used for exact shared-listener equality.
pub enum ProxyProtocolPolicyDeclaration {
    Disabled,
    Required,
    TrustedSources(Vec<String>),
}

impl From<&CanonicalProxyProtocolTrustPolicy> for ProxyProtocolPolicyDeclaration {
    fn from(policy: &CanonicalProxyProtocolTrustPolicy) -> Self {
        match policy {
            CanonicalProxyProtocolTrustPolicy::Required {} => Self::Required,
            CanonicalProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs } => {
                Self::TrustedSources(trusted_cidrs.clone())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProxyProtocolListenerContribution {
    pub listener_name: String,
    pub listener_key: ListenerKey,
    pub declaration: ProxyProtocolPolicyDeclaration,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ContributorKey {
    gateway_key: String,
    listener_name: String,
}

#[derive(Default)]
struct PolicyIndexes {
    by_gateway: HashMap<String, Vec<ProxyProtocolListenerContribution>>,
    by_listener: HashMap<ListenerKey, BTreeMap<ContributorKey, ProxyProtocolPolicyDeclaration>>,
}

#[derive(Default)]
pub struct ProxyProtocolListenerPolicyManager {
    indexes: RwLock<PolicyIndexes>,
}

impl ProxyProtocolListenerPolicyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically replace every contribution owned by one Gateway.
    ///
    /// The returned peers are limited to Gateways whose conflict set changed.
    /// An exact replay therefore performs no writes and returns no requeues.
    pub fn replace_gateway_contributions(
        &self,
        gateway_key: &str,
        mut new_contributions: Vec<ProxyProtocolListenerContribution>,
    ) -> Vec<String> {
        new_contributions.sort();
        new_contributions.dedup();

        let mut indexes = self.indexes.write();
        let old_contributions = indexes.by_gateway.get(gateway_key).cloned().unwrap_or_default();
        if old_contributions == new_contributions {
            return Vec::new();
        }

        let mut candidate_peers = peer_gateways_for_contributions(&indexes, &old_contributions, gateway_key);
        candidate_peers.extend(peer_gateways_for_contributions(
            &indexes,
            &new_contributions,
            gateway_key,
        ));
        let before: HashMap<String, BTreeSet<String>> = candidate_peers
            .iter()
            .map(|peer| (peer.clone(), conflicting_listener_names_locked(&indexes, peer)))
            .collect();

        for contribution in &old_contributions {
            let contributor = ContributorKey {
                gateway_key: gateway_key.to_string(),
                listener_name: contribution.listener_name.clone(),
            };
            if let Some(contributors) = indexes.by_listener.get_mut(&contribution.listener_key) {
                contributors.remove(&contributor);
                if contributors.is_empty() {
                    indexes.by_listener.remove(&contribution.listener_key);
                }
            }
        }

        if new_contributions.is_empty() {
            indexes.by_gateway.remove(gateway_key);
        } else {
            for contribution in &new_contributions {
                indexes
                    .by_listener
                    .entry(contribution.listener_key.clone())
                    .or_default()
                    .insert(
                        ContributorKey {
                            gateway_key: gateway_key.to_string(),
                            listener_name: contribution.listener_name.clone(),
                        },
                        contribution.declaration.clone(),
                    );
            }
            indexes.by_gateway.insert(gateway_key.to_string(), new_contributions);
        }

        candidate_peers.extend(peer_gateways_for_contributions(
            &indexes,
            indexes
                .by_gateway
                .get(gateway_key)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            gateway_key,
        ));
        candidate_peers
            .into_iter()
            .filter(|peer| {
                before.get(peer).cloned().unwrap_or_default() != conflicting_listener_names_locked(&indexes, peer)
            })
            .collect()
    }

    pub fn conflicting_listener_names(&self, gateway_key: &str) -> BTreeSet<String> {
        conflicting_listener_names_locked(&self.indexes.read(), gateway_key)
    }

    pub fn clear(&self) {
        *self.indexes.write() = PolicyIndexes::default();
    }
}

fn peer_gateways_for_contributions(
    indexes: &PolicyIndexes,
    contributions: &[ProxyProtocolListenerContribution],
    gateway_key: &str,
) -> BTreeSet<String> {
    contributions
        .iter()
        .filter_map(|contribution| indexes.by_listener.get(&contribution.listener_key))
        .flat_map(|contributors| contributors.keys())
        .filter(|contributor| contributor.gateway_key != gateway_key)
        .map(|contributor| contributor.gateway_key.clone())
        .collect()
}

fn conflicting_listener_names_locked(indexes: &PolicyIndexes, gateway_key: &str) -> BTreeSet<String> {
    indexes
        .by_gateway
        .get(gateway_key)
        .into_iter()
        .flatten()
        .filter(|contribution| {
            indexes
                .by_listener
                .get(&contribution.listener_key)
                .is_some_and(|contributors| {
                    let mut declarations = contributors.values();
                    let Some(first) = declarations.next() else {
                        return false;
                    };
                    declarations.any(|declaration| declaration != first)
                })
        })
        .map(|contribution| contribution.listener_name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn contribution(name: &str, declaration: ProxyProtocolPolicyDeclaration) -> ProxyProtocolListenerContribution {
        ProxyProtocolListenerContribution {
            listener_name: name.to_string(),
            listener_key: ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 80), "HTTP"),
            declaration,
        }
    }

    #[test]
    fn exact_replay_has_no_dirty_peers() {
        let manager = ProxyProtocolListenerPolicyManager::new();
        let entries = vec![contribution("http", ProxyProtocolPolicyDeclaration::Required)];
        assert!(manager
            .replace_gateway_contributions("default/a", entries.clone())
            .is_empty());
        assert!(manager.replace_gateway_contributions("default/a", entries).is_empty());
    }

    #[test]
    fn conflict_create_and_delete_return_only_changed_peer() {
        let manager = ProxyProtocolListenerPolicyManager::new();
        manager.replace_gateway_contributions(
            "default/a",
            vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
        );

        assert_eq!(
            manager.replace_gateway_contributions(
                "default/b",
                vec![contribution("http-b", ProxyProtocolPolicyDeclaration::Disabled)],
            ),
            vec!["default/a".to_string()]
        );
        assert_eq!(
            manager.conflicting_listener_names("default/a"),
            BTreeSet::from(["http-a".to_string()])
        );
        assert_eq!(
            manager.replace_gateway_contributions("default/b", Vec::new()),
            vec!["default/a".to_string()]
        );
        assert!(manager.conflicting_listener_names("default/a").is_empty());
    }

    #[test]
    fn declaration_change_that_preserves_conflict_has_no_dirty_peer() {
        let manager = ProxyProtocolListenerPolicyManager::new();
        manager.replace_gateway_contributions(
            "default/a",
            vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
        );
        manager.replace_gateway_contributions(
            "default/b",
            vec![contribution("http-b", ProxyProtocolPolicyDeclaration::Disabled)],
        );

        assert!(manager
            .replace_gateway_contributions(
                "default/b",
                vec![contribution(
                    "http-b",
                    ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
                )],
            )
            .is_empty());
    }

    #[test]
    fn identical_declarations_do_not_conflict_or_requeue() {
        let manager = ProxyProtocolListenerPolicyManager::new();
        manager.replace_gateway_contributions(
            "default/a",
            vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
        );
        assert!(manager
            .replace_gateway_contributions(
                "default/b",
                vec![contribution("http-b", ProxyProtocolPolicyDeclaration::Required)],
            )
            .is_empty());
        assert!(manager.conflicting_listener_names("default/a").is_empty());
        assert!(manager.conflicting_listener_names("default/b").is_empty());
    }

    #[test]
    fn mode_and_cidr_declaration_mismatches_conflict() {
        for second in [
            ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
            ProxyProtocolPolicyDeclaration::TrustedSources(vec!["198.51.100.0/24".to_string()]),
        ] {
            let manager = ProxyProtocolListenerPolicyManager::new();
            manager.replace_gateway_contributions(
                "default/a",
                vec![contribution(
                    "http-a",
                    ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
                )],
            );
            let dirty =
                manager.replace_gateway_contributions("default/b", vec![contribution("http-b", second.clone())]);
            if second == ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]) {
                assert!(dirty.is_empty());
            } else {
                assert_eq!(dirty, vec!["default/a".to_string()]);
            }
        }

        let manager = ProxyProtocolListenerPolicyManager::new();
        manager.replace_gateway_contributions(
            "default/a",
            vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
        );
        assert_eq!(
            manager.replace_gateway_contributions(
                "default/b",
                vec![contribution(
                    "http-b",
                    ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
                )],
            ),
            vec!["default/a".to_string()]
        );
    }
}

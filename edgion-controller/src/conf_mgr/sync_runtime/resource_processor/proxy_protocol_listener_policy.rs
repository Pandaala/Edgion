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
    /// Empty for the shared process; a Gateway key for a dedicated process.
    pub deployment_scope: String,
    pub declaration: ProxyProtocolPolicyDeclaration,
}

type ScopedListenerKey = (String, ListenerKey);

fn scoped_key(contribution: &ProxyProtocolListenerContribution) -> ScopedListenerKey {
    (contribution.deployment_scope.clone(), contribution.listener_key.clone())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ContributorKey {
    gateway_key: String,
    listener_name: String,
}

#[derive(Default)]
struct PolicyIndexes {
    by_gateway: HashMap<String, Vec<ProxyProtocolListenerContribution>>,
    by_listener: HashMap<ScopedListenerKey, BTreeMap<ContributorKey, ProxyProtocolPolicyDeclaration>>,
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
            let key = scoped_key(contribution);
            if let Some(contributors) = indexes.by_listener.get_mut(&key) {
                contributors.remove(&contributor);
                if contributors.is_empty() {
                    indexes.by_listener.remove(&key);
                }
            }
        }

        if new_contributions.is_empty() {
            indexes.by_gateway.remove(gateway_key);
        } else {
            for contribution in &new_contributions {
                indexes.by_listener.entry(scoped_key(contribution)).or_default().insert(
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
        .filter_map(|contribution| indexes.by_listener.get(&scoped_key(contribution)))
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
                .get(&scoped_key(contribution))
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
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/proxy_protocol_listener_policy_test.rs"]
mod tests;

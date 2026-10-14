//! Listener Port Manager
//!
//! Tracks port usage within each Gateway for intra-gateway conflict detection.
//! Per Gateway API spec, Conflicted condition applies only within a single Gateway;
//! different Gateways may independently use the same port.
//!
//! ## Design
//!
//! - Forward index: conflict_key → Set of ListenerRefs (all listeners using this socket address)
//! - Reverse index: gateway_key → Set of conflict_keys (all socket addresses used by this gateway)
//! - Conflict detection considers hostname intersection (wildcard-aware)
//!
//! ## Conflict Key Format
//!
//! Conflict keys group listeners that bind the same OS socket (same IP:port):
//! - Format: "{ip}:{port}" (e.g. "0.0.0.0:443")
//! - Protocol is excluded because HTTP/HTTPS/TLS/TCP all bind the same OS socket on a given (ip, port)
//!
//! Within each conflict key group, hostname intersection determines actual conflicts.

use super::{BidirectionalRefManager, ResourceRef};
use edgion_resources::resources::gateway::HttpServiceDeclaration;
use edgion_resources::{Gateway, ListenerKey, ResourceKind, ResourceMeta};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// Listener reference (gateway + listener name + hostname for conflict checking)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListenerRef {
    /// Gateway key in "namespace/name" format
    pub gateway_key: String,
    /// Listener name within the gateway
    pub listener_name: String,
    /// Hostname (None = wildcard, matches everything)
    pub hostname: Option<String>,
}

impl ListenerRef {
    pub fn new(gateway_key: impl Into<String>, listener_name: impl Into<String>, hostname: Option<String>) -> Self {
        Self {
            gateway_key: gateway_key.into(),
            listener_name: listener_name.into(),
            hostname,
        }
    }
}

/// Conflict group key: listeners on the same socket address can conflict.
/// Format: "{ip}:{port}" — protocol is excluded because HTTP/HTTPS/TLS/TCP
/// all bind the same OS socket on a given (ip, port).
pub type ConflictKey = String;

/// Build conflict key from a socket address.
///
/// Groups listeners by their OS socket address (ip:port) so that hostname
/// intersection can be checked within each group during conflict detection.
/// Protocol is excluded from the key because different protocols on the same
/// (ip, port) bind the same OS socket.
pub fn make_conflict_key(addr: std::net::SocketAddr) -> ConflictKey {
    addr.to_string() // e.g. "0.0.0.0:443"
}

pub type PortKey = ConflictKey;

/// Check if two listener hostnames are identical (i.e. truly conflict).
///
/// Per Gateway API spec, Conflicted means two listeners have the same
/// port+protocol+hostname combination — not merely overlapping hostnames.
/// Overlapping hostnames (e.g. wildcard vs specific) are handled by routing
/// priority and are explicitly allowed when GatewayHTTPListenerIsolation is supported.
///
/// - None == None → conflict (both are wildcard)
/// - `foo.example.com` == `foo.example.com` → conflict (identical)
/// - None vs `foo.example.com` → no conflict (different, just overlapping)
/// - `*.example.com` vs `foo.example.com` → no conflict (different, just overlapping)
fn hostnames_conflict(a: Option<&str>, b: Option<&str>) -> bool {
    a == b
}

/// A member of the first admitted compatible HTTP service group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SharedListenerMember {
    pub gateway_key: String,
    pub listener_name: String,
    pub declaration: HttpServiceDeclaration,
}
#[derive(Default)]
struct SharedListenerState {
    groups: HashMap<ListenerKey, Vec<SharedListenerMember>>,
    gateway_groups: HashMap<String, HashSet<ListenerKey>>,
    changed: HashMap<String, HashSet<String>>,
}
#[derive(serde::Serialize)]
pub struct SharedListenerStats {
    pub groups: usize,
    pub members: usize,
    pub group_keys: Vec<String>,
    pub gateway_keys: Vec<String>,
    pub pending_publications: usize,
    pub pending_gateway_keys: Vec<String>,
    pub blocker_gateways: usize,
    pub blocked_gateways: usize,
    pub blocker_references: usize,
    pub blocker_keys: Vec<String>,
    pub blocked_keys: Vec<String>,
}

/// Tracks native intra-Gateway socket conflicts and compatible shared HTTP groups.
/// Native conflicts require identical hostnames within one Gateway. Shared service
/// admission compares effective physical policy across the shared process.
pub struct ListenerPortManager {
    shared: RwLock<SharedListenerState>,
    blockers: BidirectionalRefManager<ResourceRef>,
    /// Forward index: conflict_key → listeners using this socket address
    port_to_listeners: RwLock<HashMap<ConflictKey, HashSet<ListenerRef>>>,

    /// Reverse index: gateway_key → conflict_keys it uses
    gateway_to_ports: RwLock<HashMap<String, HashSet<ConflictKey>>>,
}

impl ListenerPortManager {
    pub fn new() -> Self {
        Self {
            shared: RwLock::new(SharedListenerState::default()),
            blockers: BidirectionalRefManager::with_component("shared_listener_blockers"),
            port_to_listeners: RwLock::new(HashMap::new()),
            gateway_to_ports: RwLock::new(HashMap::new()),
        }
    }

    /// Replace this worker item's admission and complete blocking dependency set.
    /// Native listener validation has already run; compatible admitted peers remain.
    pub(crate) fn replace_shared_listeners(
        &self,
        gateway: &Gateway,
        eligible: &HashSet<String>,
    ) -> HashMap<String, Vec<SharedListenerMember>> {
        let key = gateway.key_name();
        let owner = ResourceRef::new(
            ResourceKind::Gateway,
            gateway.metadata.namespace.clone(),
            gateway.metadata.name.clone().unwrap_or_default(),
        );
        let mut state = self.shared.write();
        let mut previous = Vec::new();
        let mut affected_sources = HashSet::from([key.clone()]);
        for domain in state.gateway_groups.remove(&key).unwrap_or_default() {
            if let Some(members) = state.groups.get_mut(&domain) {
                affected_sources.extend(members.iter().map(|member| member.gateway_key.clone()));
                previous.extend(
                    members
                        .iter()
                        .filter(|member| member.gateway_key == key)
                        .cloned()
                        .map(|member| (domain.clone(), member)),
                );
                members.retain(|member| member.gateway_key != key);
                if members.is_empty() {
                    state.groups.remove(&domain);
                }
            }
        }
        self.blockers.clear_value_refs(&owner);
        let dedicated = gateway
            .metadata
            .annotations
            .as_ref()
            .and_then(|values| values.get(edgion_resources::constants::annotations::edgion::DEDICATED_GATEWAY_INSTANCE))
            .is_some_and(|value| value == "true");
        let mut incoming = Vec::new();
        let mut rejected = HashMap::new();
        let mut domains = HashSet::new();
        if !dedicated {
            for listener in gateway
                .spec
                .listeners
                .iter()
                .flatten()
                .filter(|listener| eligible.contains(&listener.name))
            {
                let Some(declaration) = HttpServiceDeclaration::from_gateway(gateway, &listener.protocol) else {
                    continue;
                };
                let domain = ListenerKey::new(
                    edgion_resources::listener_key::resolve_listener_addr(gateway, listener.port),
                    &listener.protocol,
                );
                let members = state.groups.entry(domain.clone()).or_default();
                affected_sources.extend(members.iter().map(|member| member.gateway_key.clone()));
                if members.first().is_some_and(|member| member.declaration != declaration) {
                    let blocking = members.clone();
                    for member in &blocking {
                        self.blockers.add_ref(member.gateway_key.clone(), owner.clone());
                    }
                    rejected.insert(listener.name.clone(), blocking);
                } else {
                    let member = SharedListenerMember {
                        gateway_key: key.clone(),
                        listener_name: listener.name.clone(),
                        declaration,
                    };
                    incoming.push((domain.clone(), member.clone()));
                    members.push(member);
                    domains.insert(domain);
                }
            }
        }
        if !domains.is_empty() {
            state.gateway_groups.insert(key.clone(), domains);
        }
        let normalize = |entries: &mut Vec<(ListenerKey, SharedListenerMember)>| {
            entries.sort_by(|a, b| (&a.0, &a.1.listener_name).cmp(&(&b.0, &b.1.listener_name)))
        };
        normalize(&mut previous);
        normalize(&mut incoming);
        if previous != incoming {
            state.changed.entry(key).or_default().extend(affected_sources);
        }
        rejected
    }

    pub(crate) fn published_shared_dependents(&self, key: &str) -> Vec<ResourceRef> {
        let Some(sources) = self.shared.write().changed.remove(key) else {
            return Vec::new();
        };
        sources
            .into_iter()
            .flat_map(|source| self.blockers.get_refs(&source))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }
    pub(crate) fn remove_shared_gateway(&self, gateway: &Gateway) -> Vec<ResourceRef> {
        self.replace_shared_listeners(gateway, &HashSet::new());
        let mut dependents = self
            .published_shared_dependents(&gateway.key_name())
            .into_iter()
            .collect::<HashSet<_>>();
        dependents.extend(self.blockers.clear_source_refs(&gateway.key_name()));
        dependents.into_iter().collect()
    }
    pub fn shared_stats(&self) -> SharedListenerStats {
        let state = self.shared.read();
        let mut gateway_keys = state.gateway_groups.keys().cloned().collect::<Vec<_>>();
        gateway_keys.sort();
        let mut blocker_keys = self.blockers.all_source_keys();
        blocker_keys.sort();
        let mut group_keys = state.groups.keys().map(ToString::to_string).collect::<Vec<_>>();
        group_keys.sort();
        let mut pending_gateway_keys = state.changed.keys().cloned().collect::<Vec<_>>();
        pending_gateway_keys.sort();
        let mut blocked_keys = blocker_keys
            .iter()
            .flat_map(|key| self.blockers.get_refs(key))
            .map(|value| value.resource_key())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        blocked_keys.sort();
        let refs = self.blockers.stats();
        SharedListenerStats {
            groups: state.groups.len(),
            members: state.groups.values().map(Vec::len).sum(),
            gateway_keys,
            pending_publications: state.changed.len(),
            blocker_gateways: refs.source_count,
            blocked_gateways: refs.value_count,
            blocker_references: refs.total_references,
            blocker_keys,
            group_keys,
            pending_gateway_keys,
            blocked_keys,
        }
    }

    /// Register a gateway's listeners.
    ///
    /// Each listener is a tuple of (listener_name, conflict_key, hostname).
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn register_gateway(&self, gateway_key: &str, listeners: &[(String, ConflictKey, Option<String>)]) {
        let mut gw_map = self.gateway_to_ports.write();
        let mut port_map = self.port_to_listeners.write();

        // Clear old registrations
        if let Some(old_conflict_keys) = gw_map.remove(gateway_key) {
            for conflict_key in &old_conflict_keys {
                if let Some(listeners_set) = port_map.get_mut(conflict_key) {
                    listeners_set.retain(|l| l.gateway_key != gateway_key);
                    if listeners_set.is_empty() {
                        port_map.remove(conflict_key);
                    }
                }
            }
        }

        // Add new registrations
        let mut conflict_keys = HashSet::new();

        for (listener_name, conflict_key, hostname) in listeners {
            // Normalize empty hostname to None (wildcard)
            let hostname = hostname.as_ref().filter(|h| !h.is_empty()).cloned();
            let listener_ref = ListenerRef::new(gateway_key, listener_name.clone(), hostname);
            port_map.entry(conflict_key.clone()).or_default().insert(listener_ref);
            conflict_keys.insert(conflict_key.clone());
        }

        if !conflict_keys.is_empty() {
            gw_map.insert(gateway_key.to_string(), conflict_keys);
        }

        tracing::debug!(
            gateway = %gateway_key,
            listener_count = listeners.len(),
            "Registered gateway listeners to port manager"
        );
    }

    /// Unregister a gateway and clean up all its port registrations.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn unregister_gateway(&self, gateway_key: &str) {
        let mut gw_map = self.gateway_to_ports.write();
        let mut port_map = self.port_to_listeners.write();

        let conflict_keys = gw_map.remove(gateway_key).unwrap_or_default();

        for conflict_key in &conflict_keys {
            if let Some(listeners) = port_map.get_mut(conflict_key) {
                listeners.retain(|l| l.gateway_key != gateway_key);
                if listeners.is_empty() {
                    port_map.remove(conflict_key);
                }
            }
        }

        if !conflict_keys.is_empty() {
            tracing::debug!(
                gateway = %gateway_key,
                socket_count = conflict_keys.len(),
                "Unregistered gateway from port manager"
            );
        }
    }

    /// Detect intra-gateway port conflicts for a specific gateway.
    ///
    /// Two listeners sharing a socket address conflict only if their hostnames are
    /// identical: both wildcard (`None`), or the same concrete string. Overlapping
    /// but distinct hostnames (wildcard vs concrete, or two different concrete
    /// values) are not conflicts — routing priority resolves the overlap per
    /// GatewayHTTPListenerIsolation semantics.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn detect_conflicts(&self, gateway_key: &str) -> HashMap<String, String> {
        let mut conflicts = HashMap::new();

        let gw_map = self.gateway_to_ports.read();
        let port_map = self.port_to_listeners.read();

        let Some(port_keys) = gw_map.get(gateway_key) else {
            return conflicts;
        };

        for conflict_key in port_keys {
            let Some(listeners) = port_map.get(conflict_key) else {
                continue;
            };

            // Collect same-gateway listeners on this socket address
            let same_gw: Vec<&ListenerRef> = listeners.iter().filter(|l| l.gateway_key == gateway_key).collect();

            if same_gw.len() < 2 {
                continue;
            }

            // Check hostname intersection pairwise: two listeners on the same socket address
            // conflict if they have the same hostname (including both being wildcard).
            for (i, listener) in same_gw.iter().enumerate() {
                let mut others: Vec<&str> = same_gw
                    .iter()
                    .enumerate()
                    .filter(|(j, other)| {
                        *j != i && hostnames_conflict(listener.hostname.as_deref(), other.hostname.as_deref())
                    })
                    .map(|(_, l)| l.listener_name.as_str())
                    .collect();
                // Condition messages are semantic status: hash iteration must
                // not turn a status-only WATCH into another status write.
                others.sort_unstable();

                if !others.is_empty() {
                    let reason = format!("Socket {}: conflicts with: {}", conflict_key, others.join(", "));
                    conflicts.insert(listener.listener_name.clone(), reason);
                }
            }
        }

        conflicts
    }

    /// Clear all registrations (e.g. during reload/re-election).
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn clear(&self) {
        *self.shared.write() = SharedListenerState::default();
        self.blockers.clear();
        let mut gw_map = self.gateway_to_ports.write();
        let mut port_map = self.port_to_listeners.write();
        gw_map.clear();
        port_map.clear();
        tracing::info!("ListenerPortManager cleared");
    }

    /// Get statistics for debugging/metrics
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn stats(&self) -> (usize, usize) {
        let gw_map = self.gateway_to_ports.read();
        let port_map = self.port_to_listeners.read();
        (port_map.len(), gw_map.len())
    }
}

impl Default for ListenerPortManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/listener_port_manager_test.rs"]
mod tests;

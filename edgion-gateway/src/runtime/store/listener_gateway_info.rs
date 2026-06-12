//! Listener-based GatewayInfo Store
//!
//! Maintains a dynamic mapping from listener port to all Gateway/Listener
//! contexts sharing that port. This enables correct route matching when
//! Gateways are added or removed at runtime, without requiring a restart.
//!
//! ## Why This Exists
//!
//! Route matching via `deep_match` → `check_gateway_listener_match` needs the
//! list of GatewayInfo for the request's port.  Previously this list was baked
//! into the HTTP proxy listener at startup and never updated. If a new Gateway was added
//! at runtime sharing an already-bound port, its routes would silently fail to
//! match.  This store is rebuilt on every Gateway change, keeping the list
//! current.
//!
//! ## Performance
//!
//! Uses `ArcSwap` for lock-free reads on the hot path. Rebuilds (writes) happen
//! only on Gateway create/update/delete, which is rare.

use super::super::gateway_info::GatewayInfo;
use crate::observe::metrics::TestType;
use crate::runtime::matching::route::hostname_matches_listener;
use arc_swap::ArcSwap;
use edgion_resources::constants::annotations::edgion as annotations;
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::Gateway;
use kube::ResourceExt;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

// ============================================================================
// GatewayInfoIndex — pre-built per-port gateway/listener lookup
// ============================================================================

/// Pre-indexed Gateway/Listener lookup for a single port.
/// Built once on Gateway change, used per-request with O(1) lookups.
pub struct GatewayInfoIndex {
    gateways: HashMap<String, GatewayListeners>,
}

impl GatewayInfoIndex {
    fn new() -> Self {
        Self {
            gateways: HashMap::new(),
        }
    }

    /// Fast path: gateway_key + listener_name → O(1)
    #[inline]
    pub fn get(&self, gateway_key: &str, listener_name: &str) -> Option<&ListenerEntry> {
        self.gateways.get(gateway_key).and_then(|gl| gl.get(listener_name))
    }

    /// Slow path: gateway_key → iterate listeners for hostname matching
    #[inline]
    pub fn get_gateway(&self, gateway_key: &str) -> Option<&GatewayListeners> {
        self.gateways.get(gateway_key)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.gateways.is_empty()
    }

    /// Iterate over all (gateway_key, GatewayListeners) pairs in this index.
    pub fn iter_gateways(&self) -> impl Iterator<Item = (&str, &GatewayListeners)> {
        self.gateways.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// All listeners of a single Gateway on one port, with O(1) listener_name lookup.
pub struct GatewayListeners {
    by_name: HashMap<String, ListenerEntry>,
    all: Vec<ListenerEntry>,
}

impl GatewayListeners {
    fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            all: Vec::new(),
        }
    }

    #[inline]
    pub fn get(&self, listener_name: &str) -> Option<&ListenerEntry> {
        self.by_name.get(listener_name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ListenerEntry> {
        self.all.iter()
    }

    /// Check if another listener on this gateway has a more-specific hostname
    /// that also matches `request_hostname`.
    ///
    /// Specificity: exact > wildcard > catch-all (no hostname).
    /// Since `GatewayInfoIndex` is per-port, all entries share the same port
    /// and cross-port checks are unnecessary.
    pub fn has_more_specific(&self, request_hostname: &str, current_hostname: Option<&str>) -> bool {
        if matches!(current_hostname, Some(h) if !h.starts_with("*.")) {
            return false;
        }

        for entry in &self.all {
            let other_host = match entry.hostname.as_deref() {
                Some(h) => h,
                None => continue,
            };

            if !hostname_matches_listener(request_hostname, other_host) {
                continue;
            }

            match current_hostname {
                None => {
                    return true;
                }
                Some(current_host) => {
                    if !other_host.starts_with("*.") {
                        return true;
                    }
                    if other_host != current_host && other_host.len() > current_host.len() {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn add(&mut self, entry: ListenerEntry) {
        if let Some(ref name) = entry.info.listener_name {
            self.by_name.insert(name.clone(), entry.clone());
        }
        self.all.push(entry);
    }
}

/// A single Listener's pre-built matching info.
/// Combines GatewayInfo (identity/metrics) with the Listener's hostname + allowedRoutes.
#[derive(Clone, Debug)]
pub struct ListenerEntry {
    pub info: GatewayInfo,
    /// Listener hostname constraint (None = catch-all)
    pub hostname: Option<String>,
    /// AllowedRoutes from the Listener spec (for kind/namespace filtering)
    pub allowed_routes: Option<AllowedRoutes>,
}

/// Epoch-consistent pair of the flat and indexed listener maps.
///
/// `flat` and `index` are derived from the same Gateway slice in a single
/// `rebuild()` call, and are always swapped together via a single `ArcSwap`
/// store on `ListenerGatewayInfoStore::snapshot`. Readers that need both
/// maps can `load()` once and access both fields from the resulting Arc,
/// guaranteed to belong to the same epoch.
struct ListenerGatewayInfoSnapshot {
    flat: HashMap<ListenerKey, Arc<Vec<GatewayInfo>>>,
    index: HashMap<ListenerKey, Arc<GatewayInfoIndex>>,
}

impl ListenerGatewayInfoSnapshot {
    fn empty() -> Self {
        Self {
            flat: HashMap::new(),
            index: HashMap::new(),
        }
    }
}

pub struct ListenerGatewayInfoStore {
    // Single ArcSwap so that `flat` and `index` are always swapped atomically
    // and a concurrent reader can never observe a cross-epoch pair.
    snapshot: ArcSwap<ListenerGatewayInfoSnapshot>,
}

impl ListenerGatewayInfoStore {
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(ListenerGatewayInfoSnapshot::empty()),
        }
    }

    /// Get all GatewayInfo contexts for a given listener key.
    ///
    /// Returns an empty Vec (behind Arc) if the key has no registered Gateways.
    /// This is the hot-path call used in `pg_request_filter` per request.
    #[inline]
    pub fn get(&self, key: &ListenerKey) -> Arc<Vec<GatewayInfo>> {
        let snap = self.snapshot.load();
        snap.flat.get(key).cloned().unwrap_or_else(|| Arc::new(Vec::new()))
    }

    /// Get the pre-built GatewayInfoIndex for a given listener key.
    ///
    /// Returns an empty index if the key has no registered Gateways.
    /// Hot-path call: O(1) ArcSwap load + HashMap get.
    #[inline]
    pub fn get_index(&self, key: &ListenerKey) -> Arc<GatewayInfoIndex> {
        let snap = self.snapshot.load();
        snap.index
            .get(key)
            .cloned()
            .unwrap_or_else(|| Arc::new(GatewayInfoIndex::new()))
    }

    /// Return all listener keys that have at least one registered Gateway/Listener.
    pub fn all_listener_keys(&self) -> Vec<ListenerKey> {
        let snap = self.snapshot.load();
        snap.flat.keys().cloned().collect()
    }

    /// Rebuild the store from the full list of Gateway resources.
    ///
    /// Iterates all Gateways and their Listeners, skipping conflicted ones,
    /// and builds both the flat port → Vec<GatewayInfo> mapping and the
    /// per-port GatewayInfoIndex for O(1) route matching lookups.
    pub fn rebuild(&self, gateways: &[Gateway]) {
        let mut port_map: HashMap<ListenerKey, Vec<GatewayInfo>> = HashMap::new();
        let mut index_map: HashMap<ListenerKey, GatewayInfoIndex> = HashMap::new();

        for gateway in gateways {
            let gateway_namespace = gateway.metadata.namespace.clone();
            let gateway_name = gateway.name_any();

            let listeners = match &gateway.spec.listeners {
                Some(l) => l,
                None => continue,
            };

            let gateway_annotations_map: HashMap<String, String> = gateway
                .metadata
                .annotations
                .clone()
                .map(|btree| btree.into_iter().collect())
                .unwrap_or_default();

            let metrics_test_key = gateway_annotations_map.get(annotations::METRICS_TEST_KEY).cloned();
            let metrics_test_type = gateway_annotations_map
                .get(annotations::METRICS_TEST_TYPE)
                .map(|s| TestType::from_str(s));

            let require_sni_host_match = gateway_annotations_map
                .get(annotations::REQUIRE_SNI_HOST_MATCH)
                .map(|v| v == "true");

            for listener in listeners {
                if is_listener_conflicted(gateway, &listener.name) {
                    continue;
                }
                if is_listener_not_programmed(gateway, &listener.name) {
                    continue;
                }

                let addr = resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port);
                let key = ListenerKey::new(addr, &listener.protocol);
                let gi = GatewayInfo::new(
                    gateway_namespace.clone(),
                    gateway_name.clone(),
                    Some(listener.name.clone()),
                    metrics_test_key.clone(),
                    metrics_test_type.clone(),
                    require_sni_host_match,
                );

                let gateway_key = gi.gateway_key();
                let entry = ListenerEntry {
                    info: gi.clone(),
                    hostname: listener.hostname.clone(),
                    allowed_routes: listener.allowed_routes.clone(),
                };

                index_map
                    .entry(key.clone())
                    .or_insert_with(GatewayInfoIndex::new)
                    .gateways
                    .entry(gateway_key)
                    .or_insert_with(GatewayListeners::new)
                    .add(entry);

                port_map.entry(key).or_default().push(gi);
            }
        }

        let listener_count = port_map.len();
        let total_entries: usize = port_map.values().map(|v| v.len()).sum();

        let flat_arc_map: HashMap<ListenerKey, Arc<Vec<GatewayInfo>>> = port_map
            .into_iter()
            .map(|(key, infos)| (key, Arc::new(infos)))
            .collect();

        let index_arc_map: HashMap<ListenerKey, Arc<GatewayInfoIndex>> = index_map
            .into_iter()
            .map(|(key, index)| (key, Arc::new(index)))
            .collect();

        // Single atomic swap: both `flat` and `index` become visible together,
        // so a concurrent reader can never see a cross-epoch pair.
        self.snapshot.store(Arc::new(ListenerGatewayInfoSnapshot {
            flat: flat_arc_map,
            index: index_arc_map,
        }));

        tracing::info!(
            component = "listener_gateway_info_store",
            listeners = listener_count,
            gateway_listener_entries = total_entries,
            "Rebuilt listener GatewayInfo store"
        );
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListenerGatewayInfoStats {
    pub listener_count: usize,
    pub total_entries: usize,
}

impl ListenerGatewayInfoStore {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> ListenerGatewayInfoStats {
        let snap = self.snapshot.load();
        let total: usize = snap.flat.values().map(|v| v.len()).sum();
        ListenerGatewayInfoStats {
            listener_count: snap.flat.len(),
            total_entries: total,
        }
    }
}

impl Default for ListenerGatewayInfoStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_gateway_key(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", namespace, name)
    }
}

/// Check if a Listener is marked as Conflicted in Gateway status.
///
/// Returns true if the Listener has a Conflicted condition with status "True".
/// This is set by the Controller when port conflicts are detected.
pub(crate) fn is_listener_conflicted(gateway: &Gateway, listener_name: &str) -> bool {
    gateway
        .status
        .as_ref()
        .and_then(|s| s.listeners.as_ref())
        .and_then(|listeners| listeners.iter().find(|ls| ls.name == listener_name))
        .map(|ls| {
            ls.conditions
                .iter()
                .any(|c| c.type_ == "Conflicted" && c.status == "True")
        })
        .unwrap_or(false)
}

/// Check if a Listener is explicitly marked as not programmed in Gateway status.
///
/// Returns `true` when the Listener has `Programmed=False` **or** `Accepted=False`.
/// This prevents the data plane from registering TLS and route entries for listeners
/// that the Controller has determined cannot be correctly configured (e.g. invalid CA
/// cert reference, unresolvable Secret, unsupported TLS mode).
///
/// Returns `false` (allow-through) when:
/// - Gateway has no status yet (initial creation — avoids startup race with Controller)
/// - Listener name not found in status (partial update — safe side is allow)
pub(crate) fn is_listener_not_programmed(gateway: &Gateway, listener_name: &str) -> bool {
    gateway
        .status
        .as_ref()
        .and_then(|s| s.listeners.as_ref())
        .and_then(|listeners| listeners.iter().find(|ls| ls.name == listener_name))
        .map(|ls| {
            ls.conditions
                .iter()
                .any(|c| (c.type_ == "Programmed" || c.type_ == "Accepted") && c.status == "False")
        })
        .unwrap_or(false)
}

static GLOBAL_LISTENER_GATEWAY_INFO_STORE: LazyLock<ListenerGatewayInfoStore> =
    LazyLock::new(ListenerGatewayInfoStore::new);

/// Get a reference to the global ListenerGatewayInfoStore
pub fn get_listener_gateway_info_store() -> &'static ListenerGatewayInfoStore {
    &GLOBAL_LISTENER_GATEWAY_INFO_STORE
}

/// Rebuild the global ListenerGatewayInfoStore from Gateway list
pub fn rebuild_listener_gateway_infos(gateways: &[Gateway]) {
    get_listener_gateway_info_store().rebuild(gateways);
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::gateway::{GatewaySpec, GatewayStatus, Listener, ListenerStatus};
    use kube::api::ObjectMeta;

    fn make_condition(type_: &str, status: &str) -> Condition {
        Condition {
            last_transition_time: "2024-01-01T00:00:00Z".to_string(),
            message: String::new(),
            observed_generation: None,
            reason: "Test".to_string(),
            status: status.to_string(),
            type_: type_.to_string(),
        }
    }

    fn make_listener_status(name: &str, conditions: Vec<Condition>) -> ListenerStatus {
        ListenerStatus {
            name: name.to_string(),
            supported_kinds: vec![],
            attached_routes: 0,
            conditions,
        }
    }

    fn make_gateway(listener_name: &str, conditions: Vec<Condition>) -> Gateway {
        Gateway {
            metadata: ObjectMeta {
                name: Some("test-gw".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: GatewaySpec {
                gateway_class_name: "test".to_string(),
                listeners: Some(vec![Listener {
                    name: listener_name.to_string(),
                    hostname: None,
                    port: 443,
                    protocol: "HTTPS".to_string(),
                    tls: None,
                    allowed_routes: None,
                }]),
                addresses: None,
                tls: None,
            },
            status: Some(GatewayStatus {
                addresses: None,
                conditions: None,
                listeners: Some(vec![make_listener_status(listener_name, conditions)]),
            }),
        }
    }

    #[test]
    fn test_not_programmed_no_status() {
        // status=None → allow through (startup race)
        let gw = Gateway {
            metadata: ObjectMeta::default(),
            spec: GatewaySpec {
                gateway_class_name: "t".to_string(),
                listeners: None,
                addresses: None,
                tls: None,
            },
            status: None,
        };
        assert!(!is_listener_not_programmed(&gw, "https"));
    }

    #[test]
    fn test_not_programmed_when_programmed_false() {
        let gw = make_gateway(
            "https",
            vec![
                make_condition("Accepted", "True"),
                make_condition("Programmed", "False"),
            ],
        );
        assert!(is_listener_not_programmed(&gw, "https"));
    }

    #[test]
    fn test_not_programmed_when_accepted_false() {
        let gw = make_gateway("https", vec![make_condition("Accepted", "False")]);
        assert!(is_listener_not_programmed(&gw, "https"));
    }

    #[test]
    fn test_not_programmed_false_when_all_true() {
        let gw = make_gateway(
            "https",
            vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
        );
        assert!(!is_listener_not_programmed(&gw, "https"));
    }

    #[test]
    fn test_not_programmed_false_when_listener_missing_from_status() {
        let gw = make_gateway("https", vec![make_condition("Programmed", "False")]);
        // Asking about a different listener → safe to allow through
        assert!(!is_listener_not_programmed(&gw, "http"));
    }

    #[test]
    fn test_rebuild_skips_not_programmed_listener() {
        let store = ListenerGatewayInfoStore::new();

        // Gateway with Programmed=False on its only listener
        let invalid_gw = make_gateway("https", vec![make_condition("Programmed", "False")]);
        store.rebuild(&[invalid_gw]);

        use edgion_resources::listener_key::ListenerKey;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        let key = ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443), "HTTPS");
        assert!(
            store.get_index(&key).is_empty(),
            "Programmed=False listener should not be registered in the index"
        );
    }

    #[test]
    fn test_rebuild_includes_programmed_true_listener() {
        let store = ListenerGatewayInfoStore::new();

        let valid_gw = make_gateway(
            "https",
            vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
        );
        store.rebuild(&[valid_gw]);

        use edgion_resources::listener_key::ListenerKey;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        let key = ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443), "HTTPS");
        assert!(
            !store.get_index(&key).is_empty(),
            "Programmed=True listener should be registered in the index"
        );
    }
}

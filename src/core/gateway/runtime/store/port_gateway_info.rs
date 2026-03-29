//! Port-based GatewayInfo Store
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
use crate::core::gateway::observe::metrics::TestType;
use crate::core::gateway::runtime::matching::route::hostname_matches_listener;
use crate::types::constants::annotations::edgion as annotations;
use crate::types::resources::gateway::AllowedRoutes;
use crate::types::Gateway;
use arc_swap::ArcSwap;
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
/// Combines GatewayInfo (identity/metrics) + ListenerConfig (hostname + allowedRoutes).
#[derive(Clone, Debug)]
pub struct ListenerEntry {
    pub info: GatewayInfo,
    /// Listener hostname constraint (None = catch-all)
    pub hostname: Option<String>,
    /// AllowedRoutes from the Listener spec (for kind/namespace filtering)
    pub allowed_routes: Option<AllowedRoutes>,
}

pub struct PortGatewayInfoStore {
    flat_data: ArcSwap<HashMap<u16, Arc<Vec<GatewayInfo>>>>,
    index_data: ArcSwap<HashMap<u16, Arc<GatewayInfoIndex>>>,
}

impl PortGatewayInfoStore {
    pub fn new() -> Self {
        Self {
            flat_data: ArcSwap::from_pointee(HashMap::new()),
            index_data: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Get all GatewayInfo contexts for a given port.
    ///
    /// Returns an empty Vec (behind Arc) if the port has no registered Gateways.
    /// This is the hot-path call used in `pg_request_filter` per request.
    #[inline]
    pub fn get(&self, port: u16) -> Arc<Vec<GatewayInfo>> {
        let data = self.flat_data.load();
        data.get(&port).cloned().unwrap_or_else(|| Arc::new(Vec::new()))
    }

    /// Get the pre-built GatewayInfoIndex for a given port.
    ///
    /// Returns an empty index if the port has no registered Gateways.
    /// Hot-path call: O(1) ArcSwap load + HashMap get.
    #[inline]
    pub fn get_index(&self, port: u16) -> Arc<GatewayInfoIndex> {
        let data = self.index_data.load();
        data.get(&port)
            .cloned()
            .unwrap_or_else(|| Arc::new(GatewayInfoIndex::new()))
    }

    /// Return all ports that have at least one registered Gateway/Listener.
    pub fn all_ports(&self) -> Vec<u16> {
        let data = self.flat_data.load();
        data.keys().copied().collect()
    }

    /// Rebuild the store from the full list of Gateway resources.
    ///
    /// Iterates all Gateways and their Listeners, skipping conflicted ones,
    /// and builds both the flat port → Vec<GatewayInfo> mapping and the
    /// per-port GatewayInfoIndex for O(1) route matching lookups.
    pub fn rebuild(&self, gateways: &[Gateway]) {
        let mut port_map: HashMap<u16, Vec<GatewayInfo>> = HashMap::new();
        let mut index_map: HashMap<u16, GatewayInfoIndex> = HashMap::new();

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

                let port = listener.port as u16;
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
                    .entry(port)
                    .or_insert_with(GatewayInfoIndex::new)
                    .gateways
                    .entry(gateway_key)
                    .or_insert_with(GatewayListeners::new)
                    .add(entry);

                port_map.entry(port).or_default().push(gi);
            }
        }

        let port_count = port_map.len();
        let total_entries: usize = port_map.values().map(|v| v.len()).sum();

        let flat_arc_map: HashMap<u16, Arc<Vec<GatewayInfo>>> = port_map
            .into_iter()
            .map(|(port, infos)| (port, Arc::new(infos)))
            .collect();

        let index_arc_map: HashMap<u16, Arc<GatewayInfoIndex>> = index_map
            .into_iter()
            .map(|(port, index)| (port, Arc::new(index)))
            .collect();

        self.flat_data.store(Arc::new(flat_arc_map));
        self.index_data.store(Arc::new(index_arc_map));

        tracing::info!(
            component = "port_gateway_info_store",
            ports = port_count,
            gateway_listener_entries = total_entries,
            "Rebuilt port GatewayInfo store"
        );
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PortGatewayInfoStats {
    pub port_count: usize,
    pub total_entries: usize,
}

impl PortGatewayInfoStore {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> PortGatewayInfoStats {
        let data = self.flat_data.load();
        let total: usize = data.values().map(|v| v.len()).sum();
        PortGatewayInfoStats {
            port_count: data.len(),
            total_entries: total,
        }
    }
}

impl Default for PortGatewayInfoStore {
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
fn is_listener_conflicted(gateway: &Gateway, listener_name: &str) -> bool {
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

static GLOBAL_PORT_GATEWAY_INFO_STORE: LazyLock<PortGatewayInfoStore> = LazyLock::new(PortGatewayInfoStore::new);

/// Get a reference to the global PortGatewayInfoStore
pub fn get_port_gateway_info_store() -> &'static PortGatewayInfoStore {
    &GLOBAL_PORT_GATEWAY_INFO_STORE
}

/// Rebuild the global PortGatewayInfoStore from Gateway list
pub fn rebuild_port_gateway_infos(gateways: &[Gateway]) {
    get_port_gateway_info_store().rebuild(gateways);
}

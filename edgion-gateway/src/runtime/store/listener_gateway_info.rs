//! Listener-based GatewayInfo Store
//!
//! Maintains a dynamic mapping from listener port to all Gateway/Listener
//! contexts sharing that port. This enables correct route matching when
//! Gateways are added or removed at runtime, without requiring a restart.
//!
//! ## Why This Exists
//!
//! Route matching via `deep_match` → `check_gateway_listener_match` needs the
//! GatewayInfo list for the request's listener port. Baking this list into the
//! proxy listener at startup would cause a Gateway added after startup (sharing
//! an already-bound port) to silently fail route matching. This store is rebuilt
//! on every Gateway change, keeping the list current.
//!
//! ## Performance
//!
//! Uses `ArcSwap` for lock-free reads on the hot path. Rebuilds (writes) happen
//! only on Gateway create/update/delete, which is rare.

use super::super::gateway_info::GatewayInfo;
use crate::observe::metrics::TestType;
use crate::runtime::matching::route::hostname_matches_listener;
use arc_swap::ArcSwap;
use edgion_common::dns::resolver_select::candidate_from_annotations;
use edgion_resources::constants::annotations::edgion as annotations;
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::resources::edgion_tls::GatewayAttachmentProof;
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::resources::gateway::{gateway_attachment_proof, gateway_listener_is_explicitly_ineligible};
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

    /// Compare the whole admitted logical inventory for this physical key.
    /// Empty-route and policy-unavailable owners still constrain eligibility;
    /// neither permits a Route to escape to a less-specific Listener.
    pub fn has_more_specific(&self, request_hostname: &str, current_hostname: Option<&str>) -> bool {
        // Exact already has maximum specificity, regardless of serving-set size.
        if matches!(current_hostname, Some(host) if !host.starts_with("*.")) {
            return false;
        }
        self.gateways
            .values()
            .any(|listeners| listeners.has_more_specific(request_hostname, current_hostname))
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.gateways.is_empty()
    }

    /// Iterate over all (gateway_key, GatewayListeners) pairs in this index.
    pub fn iter_gateways(&self) -> impl Iterator<Item = (&str, &GatewayListeners)> {
        self.gateways.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Build an index from owned listener facts without reading another store.
    pub(crate) fn from_entries(entries: Vec<(String, ListenerEntry)>) -> Self {
        let mut index = Self::new();
        for (gateway_key, entry) in entries {
            index
                .gateways
                .entry(gateway_key)
                .or_insert_with(GatewayListeners::new)
                .add(entry);
        }
        index
    }

    #[cfg(test)]
    pub(crate) fn from_entries_for_test(entries: Vec<(String, ListenerEntry)>) -> Self {
        Self::from_entries(entries)
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
            let Some(other_host) = entry.hostname.as_deref() else {
                continue;
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
    /// Declared Gateway listener port, retained independently of the physical key.
    pub port: i32,
    pub info: Arc<GatewayInfo>,
    /// Proof for the projected Gateway generation that produced this listener.
    /// `None` is used by non-authoritative synthetic indices and cannot
    /// authorize ACME challenge serving.
    pub gateway_proof: Option<GatewayAttachmentProof>,
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
    flat: HashMap<ListenerKey, Arc<[Arc<GatewayInfo>]>>,
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
    // and one snapshot load observes an epoch-consistent pair.
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
    /// Listener construction and diagnostics use this flat view; request route
    /// matching uses `get_index` below.
    #[inline]
    pub fn get(&self, key: &ListenerKey) -> Arc<[Arc<GatewayInfo>]> {
        let snap = self.snapshot.load();
        snap.flat.get(key).cloned().unwrap_or_else(|| Arc::from(Vec::new()))
    }

    /// Get the pre-built GatewayInfoIndex for a given listener key.
    ///
    /// Returns an empty index if the key has no registered Gateways.
    /// Hot-path call: O(1) ArcSwap load + HashMap get.
    #[inline]
    pub fn get_index(&self, key: &ListenerKey) -> Arc<GatewayInfoIndex> {
        #[cfg(test)]
        if let Some(index) = TEST_INDEX_OVERRIDES.with(|overrides| overrides.borrow().get(key).cloned()) {
            return index;
        }
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
    /// and builds both the flat port → Vec<Arc<GatewayInfo>> mapping and the
    /// per-port GatewayInfoIndex for O(1) route matching lookups.
    pub fn rebuild(&self, gateways: &[Gateway]) {
        let mut port_map: HashMap<ListenerKey, Vec<Arc<GatewayInfo>>> = HashMap::new();
        let mut index_map: HashMap<ListenerKey, GatewayInfoIndex> = HashMap::new();

        for gateway in gateways {
            let gateway_namespace = gateway.metadata.namespace.clone();
            let gateway_name = gateway.name_any();

            let Some(listeners) = &gateway.spec.listeners else {
                continue;
            };

            let gateway_annotations = gateway.metadata.annotations.as_ref();

            let metrics_test_key = gateway_annotations
                .and_then(|map| map.get(annotations::METRICS_TEST_KEY))
                .cloned();
            let metrics_test_type = gateway_annotations
                .and_then(|map| map.get(annotations::METRICS_TEST_TYPE))
                .map(|s| TestType::from_str(s));

            let require_sni_host_match = gateway_annotations
                .and_then(|map| map.get(annotations::REQUIRE_SNI_HOST_MATCH))
                .map(|v| v == "true");

            let plugin_store_keys = gateway_annotations
                .and_then(|map| map.get(annotations::EDGION_PLUGINS))
                .map(|v| annotations::parse_plugin_ref_list(v, gateway_namespace.as_deref().unwrap_or("default")))
                .unwrap_or_default();

            // Precompute the gateway-layer DNS resolver candidate from the two
            // `edgion.io/dns-resolver{,-ref}` annotations, so the hot ExternalName
            // self-resolution path reads it off GatewayInfo without re-parsing.
            let dns_resolver_candidate = gateway_annotations.and_then(candidate_from_annotations);

            for listener in listeners {
                if gateway_listener_is_explicitly_ineligible(gateway, &listener.name) {
                    continue;
                }

                let addr = resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port);
                let key = ListenerKey::new(addr, &listener.protocol);
                let gi = Arc::new(GatewayInfo::new(
                    gateway_namespace.clone(),
                    gateway_name.clone(),
                    Some(listener.name.clone()),
                    metrics_test_key.clone(),
                    metrics_test_type.clone(),
                    require_sni_host_match,
                    plugin_store_keys.clone(),
                    dns_resolver_candidate.clone(),
                ));

                let gateway_key = gi.gateway_key().to_owned();
                let entry = ListenerEntry {
                    port: listener.port,
                    info: Arc::clone(&gi),
                    gateway_proof: Some(gateway_attachment_proof(gateway)),
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

        let flat_arc_map: HashMap<ListenerKey, Arc<[Arc<GatewayInfo>]>> = port_map
            .into_iter()
            .map(|(key, infos)| (key, Arc::from(infos)))
            .collect();

        let index_arc_map: HashMap<ListenerKey, Arc<GatewayInfoIndex>> = index_map
            .into_iter()
            .map(|(key, index)| (key, Arc::new(index)))
            .collect();

        // Single atomic swap: both `flat` and `index` become visible together,
        // so one snapshot load sees an epoch-consistent pair.
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

#[cfg(test)]
include!("../../../tests/support/runtime/store/listener_gateway_info_test/367_macro_invocation.rs");

#[cfg(test)]
include!("../../../tests/support/runtime/store/listener_gateway_info_test/375_struct_item.rs");

#[cfg(test)]
include!("../../../tests/support/runtime/store/listener_gateway_info_test/382_impl_item.rs");

#[cfg(test)]
include!("../../../tests/support/runtime/store/listener_gateway_info_test/394_impl_item.rs");

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

pub(crate) fn is_listener_conflicted(gateway: &Gateway, listener_name: &str) -> bool {
    edgion_resources::resources::gateway::gateway_listener_is_conflicted(gateway, listener_name)
}

pub(crate) fn is_listener_not_programmed(gateway: &Gateway, listener_name: &str) -> bool {
    edgion_resources::resources::gateway::gateway_listener_is_not_programmed(gateway, listener_name)
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
#[path = "../../../tests/unit/runtime/store/listener_gateway_info_test.rs"]
mod tests;

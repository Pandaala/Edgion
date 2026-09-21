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
thread_local! {
    static TEST_INDEX_OVERRIDES: std::cell::RefCell<HashMap<ListenerKey, Arc<GatewayInfoIndex>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Isolate actual request-entry tests without mutating a process-global snapshot.
/// The guard must remain on its creating thread, including across async awaits.
#[cfg(test)]
pub(crate) struct TestListenerIndexGuard {
    key: ListenerKey,
    previous: Option<Arc<GatewayInfoIndex>>,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(test)]
impl TestListenerIndexGuard {
    pub(crate) fn install(key: ListenerKey, index: Arc<GatewayInfoIndex>) -> Self {
        let previous = TEST_INDEX_OVERRIDES.with(|overrides| overrides.borrow_mut().insert(key.clone(), index));
        Self {
            key,
            previous,
            _thread_bound: std::marker::PhantomData,
        }
    }
}

#[cfg(test)]
impl Drop for TestListenerIndexGuard {
    fn drop(&mut self) {
        TEST_INDEX_OVERRIDES.with(|overrides| {
            let mut overrides = overrides.borrow_mut();
            if let Some(previous) = self.previous.take() {
                overrides.insert(self.key.clone(), previous);
            } else {
                overrides.remove(&self.key);
            }
        });
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
mod tests {
    use super::*;
    use edgion_common::dns::resolver_select::{LinkSysKey, ResolverCandidate};
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::gateway::{GatewaySpec, GatewayStatus, Listener, ListenerStatus};
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;

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
        crate::test_current_status::processed(Gateway {
            metadata: ObjectMeta {
                name: Some("test-gw".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: GatewaySpec {
                current_status: None,
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
                resolved_inbound_proxy_protocol: Default::default(),
                resolved_attachment_proof: None,
            },
            status: Some(GatewayStatus {
                addresses: None,
                conditions: None,
                listeners: Some(vec![make_listener_status(listener_name, conditions)]),
            }),
        })
    }

    #[test]
    fn annotation_projection_preserves_absent_empty_and_unrelated_maps() {
        for annotations in [
            None,
            Some(BTreeMap::new()),
            Some(BTreeMap::from([(
                "example.com/unrelated".to_owned(),
                "ignored".repeat(128),
            )])),
        ] {
            let store = ListenerGatewayInfoStore::new();
            let mut gateway = make_gateway("https", vec![]);
            gateway.metadata.annotations = annotations;
            store.rebuild(&[gateway]);
            let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
            let flat = store.get(&key);
            assert_eq!(flat.len(), 1);
            let info = &flat[0];
            assert_eq!(info.metrics_test_key, None);
            assert_eq!(info.metrics_test_type, None);
            assert_eq!(info.require_sni_host_match, None);
            assert!(info.plugin_store_keys.is_empty());
            assert_eq!(info.dns_resolver_candidate, None);
        }
    }

    #[test]
    fn annotation_projection_preserves_metrics_and_exact_sni_boolean() {
        for (sni, expected_sni, metrics_type, expected_type) in [
            ("true", true, "LB", TestType::Lb),
            ("false", false, "retry", TestType::Retry),
            ("True", false, "LATENCY", TestType::Latency),
            (" true ", false, "unknown", TestType::None),
            ("", false, "", TestType::None),
        ] {
            let store = ListenerGatewayInfoStore::new();
            let mut gateway = make_gateway("https", vec![]);
            gateway.metadata.annotations = Some(BTreeMap::from([
                (annotations::METRICS_TEST_KEY.to_owned(), sni.to_owned()),
                (annotations::METRICS_TEST_TYPE.to_owned(), metrics_type.to_owned()),
                (annotations::REQUIRE_SNI_HOST_MATCH.to_owned(), sni.to_owned()),
                ("example.com/unrelated".to_owned(), "ignored".to_owned()),
            ]));
            store.rebuild(&[gateway]);
            let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
            let flat = store.get(&key);
            let info = &flat[0];
            assert_eq!(info.metrics_test_key.as_deref(), Some(sni));
            assert_eq!(info.metrics_test_type, Some(expected_type));
            assert_eq!(info.require_sni_host_match, Some(expected_sni));
        }
    }

    #[test]
    fn annotation_projection_preserves_plugin_namespace_fallback() {
        for (namespace, expected) in [
            (None, "default/local"),
            (Some(""), "/local"),
            (Some("team"), "team/local"),
        ] {
            for (plugins, expected_keys) in [
                ("local, shared/plugin local", vec![expected, "shared/plugin"]),
                ("", vec![]),
            ] {
                let store = ListenerGatewayInfoStore::new();
                let mut gateway = make_gateway("https", vec![]);
                gateway.metadata.namespace = namespace.map(str::to_owned);
                gateway.metadata.annotations = Some(BTreeMap::from([(
                    annotations::EDGION_PLUGINS.to_owned(),
                    plugins.to_owned(),
                )]));
                store.rebuild(&[gateway]);
                let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
                let flat = store.get(&key);
                assert_eq!(flat[0].plugin_store_keys, expected_keys);
                assert_eq!(flat[0].namespace(), namespace);
            }
        }
    }

    #[test]
    fn annotation_projection_preserves_dns_candidate_precedence() {
        let servers = ResolverCandidate::Servers(vec!["1.1.1.1".to_owned(), "8.8.8.8:5353".to_owned()]);
        let reference = ResolverCandidate::HttpDnsRef(LinkSysKey {
            namespace: "dns".to_owned(),
            name: "resolver".to_owned(),
        });
        for (inline, dns_ref, expected) in [
            (Some("1.1.1.1, 8.8.8.8:5353\t1.1.1.1"), None, Some(servers.clone())),
            (None, Some(" dns / resolver "), Some(reference.clone())),
            (Some("9.9.9.9"), Some("dns/resolver"), Some(reference)),
            (Some("1.1.1.1\n8.8.8.8:5353"), Some("bare-name"), Some(servers)),
            (None, Some("bare-name"), None),
            (Some(" , \t\n"), Some(""), None),
        ] {
            let store = ListenerGatewayInfoStore::new();
            let mut gateway = make_gateway("https", vec![]);
            let mut map = BTreeMap::from([("example.com/unrelated".to_owned(), "ignored".to_owned())]);
            for (key, value) in [
                (annotations::DNS_RESOLVER, inline),
                (annotations::DNS_RESOLVER_REF, dns_ref),
            ] {
                if let Some(value) = value {
                    map.insert(key.to_owned(), value.to_owned());
                }
            }
            gateway.metadata.annotations = Some(map);
            store.rebuild(&[gateway]);
            let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
            let flat = store.get(&key);
            assert_eq!(flat[0].dns_resolver_candidate, expected);
        }
    }

    #[test]
    fn test_not_programmed_no_status() {
        // Missing Controller projection cannot authorize a listener.
        let gw = Gateway {
            metadata: ObjectMeta::default(),
            spec: GatewaySpec {
                current_status: None,
                gateway_class_name: "t".to_string(),
                listeners: None,
                addresses: None,
                tls: None,
                resolved_inbound_proxy_protocol: Default::default(),
                resolved_attachment_proof: None,
            },
            status: None,
        };
        assert!(gateway_listener_is_explicitly_ineligible(&gw, "https"));
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
        assert!(gateway_listener_is_explicitly_ineligible(&gw, "https"));
    }

    #[test]
    fn test_not_programmed_when_accepted_false() {
        let gw = make_gateway("https", vec![make_condition("Accepted", "False")]);
        assert!(gateway_listener_is_explicitly_ineligible(&gw, "https"));
    }

    #[test]
    fn test_not_programmed_false_when_all_true() {
        let gw = make_gateway(
            "https",
            vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
        );
        assert!(!gateway_listener_is_explicitly_ineligible(&gw, "https"));
    }

    #[test]
    fn test_listener_missing_from_current_status_is_ineligible() {
        let gw = make_gateway("https", vec![make_condition("Programmed", "False")]);
        assert!(gateway_listener_is_explicitly_ineligible(&gw, "http"));
    }

    #[test]
    fn listener_publication_uses_only_current_status() {
        let store = ListenerGatewayInfoStore::new();
        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let mut gateway = make_gateway("https", vec![make_condition("Accepted", "True")]);
        let allowed = gateway.spec.current_status.clone();
        let rejected = make_gateway("https", vec![make_condition("Accepted", "False")])
            .spec
            .current_status;

        gateway.status = rejected.clone();
        store.rebuild(&[gateway.clone()]);
        assert!(!store.get_index(&key).is_empty());

        gateway.status = allowed.clone();
        gateway.spec.current_status = rejected;
        store.rebuild(&[gateway.clone()]);
        assert!(store.get_index(&key).is_empty());

        gateway.spec.current_status = None;
        store.rebuild(&[gateway.clone()]);
        assert!(store.get_index(&key).is_empty());

        gateway.spec.current_status = allowed;
        store.rebuild(&[gateway]);
        assert!(!store.get_index(&key).is_empty());
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

    #[test]
    fn flat_and_index_share_the_same_gateway_info() {
        let store = ListenerGatewayInfoStore::new();
        let gateway = make_gateway(
            "https",
            vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
        );
        store.rebuild(&[gateway]);

        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let flat = store.get(&key);
        let index = store.get_index(&key);
        let indexed = index.get("default/test-gw", "https").unwrap();

        assert_eq!(flat.len(), 1);
        assert!(Arc::ptr_eq(&flat[0], &indexed.info));
        assert_eq!(flat[0].gateway_key(), "default/test-gw");
        assert_eq!(flat[0].gateway_key().as_ptr(), indexed.info.gateway_key().as_ptr());
    }

    #[test]
    fn index_carries_the_gateway_attachment_proof_from_the_same_snapshot() {
        let store = ListenerGatewayInfoStore::new();
        let mut gateway = make_gateway(
            "https",
            vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
        );
        let proof = GatewayAttachmentProof {
            instance: "controller-a".to_string(),
            generation: 7,
            epoch: "epoch-a".to_string(),
            attachment_digest: "a".repeat(64),
        };
        gateway.spec.resolved_attachment_proof = Some(proof.clone());
        store.rebuild(&[gateway]);

        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let index = store.get_index(&key);
        let entry = index.get("default/test-gw", "https").unwrap();
        assert_eq!(entry.gateway_proof.as_ref(), Some(&proof));
    }

    #[test]
    fn in_flight_gateway_info_survives_generation_replacement() {
        let store = ListenerGatewayInfoStore::new();
        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let mut gateway = make_gateway(
            "https",
            vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
        );
        gateway.metadata.annotations = Some(std::collections::BTreeMap::from([(
            annotations::METRICS_TEST_KEY.to_string(),
            "old-generation".to_string(),
        )]));
        store.rebuild(&[gateway.clone()]);
        let old = Arc::clone(&store.get(&key)[0]);

        gateway
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(annotations::METRICS_TEST_KEY.to_string(), "new-generation".to_string());
        store.rebuild(&[gateway]);
        let new = Arc::clone(&store.get(&key)[0]);

        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(old.metrics_test_key.as_deref(), Some("old-generation"));
        assert_eq!(new.metrics_test_key.as_deref(), Some("new-generation"));
        assert_eq!(old.gateway_key(), "default/test-gw");
        assert_eq!(new.gateway_key(), "default/test-gw");
    }

    #[test]
    fn listener_port_replacement_preserves_the_old_index_snapshot() {
        let store = ListenerGatewayInfoStore::new();
        let mut gateway = make_gateway("https", vec![]);
        let old_key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let new_key = ListenerKey::new("0.0.0.0:8443".parse().unwrap(), "HTTPS");
        store.rebuild(&[gateway.clone()]);
        let old = store.get_index(&old_key);
        assert_eq!(old.get("default/test-gw", "https").unwrap().port, 443);
        gateway.spec.listeners.as_mut().unwrap()[0].port = 8443;
        store.rebuild(&[gateway]);
        assert!(store.get_index(&old_key).is_empty());
        assert_eq!(
            store.get_index(&new_key).get("default/test-gw", "https").unwrap().port,
            8443
        );
        assert_eq!(old.get("default/test-gw", "https").unwrap().port, 443);
    }

    #[test]
    fn retained_gateway_identity_survives_store_replacement() {
        let store = ListenerGatewayInfoStore::new();
        let key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let mut gateway = make_gateway(
            "https",
            vec![make_condition("Accepted", "True"), make_condition("Programmed", "True")],
        );
        store.rebuild(&[gateway.clone()]);
        let old = Arc::clone(&store.get(&key)[0]);
        gateway.metadata.namespace = Some("new-namespace".to_owned());
        gateway.metadata.name = Some("new-gateway".to_owned());
        store.rebuild(&[gateway]);

        let index = store.get_index(&key);
        assert!(index.get("default/test-gw", "https").is_none());
        let new = &index.get("new-namespace/new-gateway", "https").unwrap().info;
        assert_eq!(old.namespace(), Some("default"));
        assert_eq!(old.gateway_name(), "test-gw");
        assert_eq!(old.gateway_key(), "default/test-gw");
        assert_eq!(new.namespace(), Some("new-namespace"));
        assert_eq!(new.gateway_name(), "new-gateway");
        assert_eq!(new.gateway_key(), "new-namespace/new-gateway");
        assert!(!Arc::ptr_eq(&old, new));
    }
}

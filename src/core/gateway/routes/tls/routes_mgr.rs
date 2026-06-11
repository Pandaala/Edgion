use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::tls::gateway_tls_routes::TlsRouteTable;
use crate::core::gateway::routes::tls::route_unit::TlsRouteUnit;
use edgion_resources::resources::TLSRoute;
use edgion_resources::ListenerKey;

/// Per-listener TLS route manager.
///
/// Each instance owns an atomically-swappable `TlsRouteTable` snapshot.
/// `EdgionTlsTcpProxy` holds an `Arc<TlsRouteManager>` and calls
/// `load_route_table()` per-connection for lock-free lookups.
///
/// Route data is owned by `GlobalTlsRouteManagers`; this struct only
/// holds the compiled hostname-index snapshot for one listener.
pub struct TlsRouteManager {
    route_table: ArcSwap<TlsRouteTable>,
}

impl Default for TlsRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsRouteManager {
    pub fn new() -> Self {
        Self {
            route_table: ArcSwap::from_pointee(TlsRouteTable::new()),
        }
    }

    /// Load the current route table snapshot (hot path, per-connection).
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<TlsRouteTable>> {
        self.route_table.load()
    }

    /// Rebuild the route table from units belonging to this listener.
    /// Called by `GlobalTlsRouteManagers` during rebuild.
    pub fn rebuild(&self, units: &HashMap<String, Arc<TlsRouteUnit>>) {
        let new_table = TlsRouteTable::from_units(units);

        tracing::debug!(
            component = "tls_route_manager",
            routes = units.len(),
            catch_all = new_table.has_catch_all(),
            "Rebuilt per-listener TLS route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `ListenerKey -> Arc<TlsRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> Arc<TLSRoute>`) and
/// implements `ConfHandler<TLSRoute>`.  On every change it rebuilds all
/// per-listener managers by resolving each route's target listeners from its
/// `spec.resolved_listeners`.
pub struct GlobalTlsRouteManagers {
    /// Canonical route store: resource_key -> compiled Arc<TlsRouteUnit>.
    ///
    /// Units (and their `backend_finder` WRR cursor) are built ONCE in the
    /// conf-handler and reused across listener rebuilds, so an unrelated route
    /// change on the same listener does not reset a sibling route's selector.
    /// A single unit is shared across all SNI hostname buckets of its route.
    route_cache: DashMap<String, Arc<TlsRouteUnit>>,

    /// listener -> per-listener manager (stable Arc held by listeners)
    by_listener: DashMap<ListenerKey, Arc<TlsRouteManager>>,

    /// Serializes the read-snapshot-then-rewrite sequences that mutate
    /// `route_cache` + `by_listener`. The TLSRoute cache task
    /// (`full_set`/`partial_update`) and the Gateway cache task
    /// (`rebuild_all_listener_managers` via `runtime::handler`) reach this manager
    /// concurrently under different per-kind locks; this is the only lock both
    /// contend on, preventing a stale-snapshot rebuild leak. Synchronous only.
    rebuild_lock: parking_lot::Mutex<()>,
}

impl Default for GlobalTlsRouteManagers {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalTlsRouteManagers {
    pub fn new() -> Self {
        Self {
            route_cache: DashMap::new(),
            by_listener: DashMap::new(),
            rebuild_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Acquire the rebuild lock. Held for the whole body of `full_set`,
    /// `partial_update`, and the public `rebuild_all_listener_managers`.
    /// NOT reentrant — internal callers use the `*_locked` variants.
    pub(crate) fn lock_rebuild(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.rebuild_lock.lock()
    }

    /// Get or create a per-listener `TlsRouteManager`.
    ///
    /// Called by `listener_builder` at startup. The returned `Arc` is stable;
    /// route updates only swap the inner `ArcSwap<TlsRouteTable>`.
    pub fn get_or_create_listener_manager(&self, key: &ListenerKey) -> Arc<TlsRouteManager> {
        self.by_listener
            .entry(key.clone())
            .or_insert_with(|| Arc::new(TlsRouteManager::new()))
            .value()
            .clone()
    }

    /// Insert a compiled route unit into the cache.
    pub fn insert_route(&self, unit: Arc<TlsRouteUnit>) {
        let key = unit.key_name();
        self.route_cache.insert(key, unit);
    }

    /// Get a route unit from the cache by key.
    pub fn get_route(&self, key: &str) -> Option<Arc<TlsRouteUnit>> {
        self.route_cache.get(key).map(|entry| entry.value().clone())
    }

    /// Remove a route from the cache by resource key.
    pub fn remove_route(&self, resource_key: &str) {
        self.route_cache.remove(resource_key);
    }

    /// Clear all routes from the cache.
    pub fn clear_route_cache(&self) {
        self.route_cache.clear();
    }

    /// Rebuild all per-listener managers from the current route cache.
    ///
    /// Delegates to the shared L4 orchestrator
    /// `routes::l4_rebuild::rebuild_all_l4_listener_managers` (see that
    /// function's docs for invariants — bucket / rebuild / stale-sweep /
    /// clear-pass).
    pub fn rebuild_all_listener_managers(&self) {
        let _guard = self.rebuild_lock.lock();
        self.rebuild_all_listener_managers_locked();
    }

    /// Rebuild body. The caller MUST hold `rebuild_lock`.
    pub(crate) fn rebuild_all_listener_managers_locked(&self) {
        let active = crate::core::gateway::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::core::gateway::routes::l4_rebuild::rebuild_all_l4_listener_managers(
            "global_tls_route_managers",
            &self.by_listener,
            &self.route_cache,
            active,
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |k| self.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
        );
    }

    /// Rebuild only the per-listener managers for the given set of affected
    /// listeners. Delegates to the shared L4 orchestrator
    /// `routes::l4_rebuild::rebuild_affected_l4_listener_managers`.
    pub fn rebuild_affected_listener_managers(&self, affected_listeners: &HashSet<ListenerKey>) {
        let active = crate::core::gateway::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::core::gateway::routes::l4_rebuild::rebuild_affected_l4_listener_managers(
            "global_tls_route_managers",
            &self.by_listener,
            &self.route_cache,
            affected_listeners,
            active,
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |k| self.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
        );
    }
}

/// Get the resolved listeners for a TLSRoute.
///
/// Uses `spec.resolved_listeners` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.
pub(crate) fn resolved_listeners_for_route(route: &TLSRoute) -> &[ListenerKey] {
    route.spec.resolved_listeners.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TlsRouteManagerStats {
    pub route_cache: usize,
    pub listener_count: usize,
}

impl GlobalTlsRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> TlsRouteManagerStats {
        TlsRouteManagerStats {
            route_cache: self.route_cache.len(),
            listener_count: self.by_listener.len(),
        }
    }
}

static GLOBAL_TLS_ROUTE_MANAGERS: OnceLock<GlobalTlsRouteManagers> = OnceLock::new();

pub fn get_global_tls_route_managers() -> &'static GlobalTlsRouteManagers {
    GLOBAL_TLS_ROUTE_MANAGERS.get_or_init(GlobalTlsRouteManagers::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::routes::l4_rebuild::{
        rebuild_affected_l4_listener_managers, rebuild_all_l4_listener_managers,
    };
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::resources::tls_route::TLSRouteSpec;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_tls_route(namespace: &str, name: &str, listener_key: ListenerKey) -> TLSRoute {
        TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: Some("tls".to_string()),
                    port: None,
                }]),
                hostnames: None,
                rules: Some(vec![]),
                resolved_listeners: Some(vec![listener_key]),
                effective_hostnames: None,
            },
            status: None,
        }
    }

    fn test_unit(namespace: &str, name: &str, listener_key: ListenerKey) -> Arc<TlsRouteUnit> {
        Arc::new(TlsRouteUnit::from_route(Arc::new(create_test_tls_route(
            namespace,
            name,
            listener_key,
        ))))
    }

    /// Drive the shared helper directly with a synthetic `active` PGIS
    /// snapshot so tests do not race against the global store.
    fn drive_rebuild_all(managers: &GlobalTlsRouteManagers, active: Vec<ListenerKey>) {
        rebuild_all_l4_listener_managers(
            "global_tls_route_managers",
            &managers.by_listener,
            &managers.route_cache,
            active,
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |k| managers.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
        );
    }

    fn drive_rebuild_affected(
        managers: &GlobalTlsRouteManagers,
        affected: &HashSet<ListenerKey>,
        active: Vec<ListenerKey>,
    ) {
        rebuild_affected_l4_listener_managers(
            "global_tls_route_managers",
            &managers.by_listener,
            &managers.route_cache,
            affected,
            active,
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |k| managers.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
        );
    }

    /// Regression for F-007: `rebuild_all` must clear the route table of an
    /// active listener that previously had routes but now has none.
    #[test]
    fn test_rebuild_all_clears_active_listener_with_no_routes() {
        let managers = GlobalTlsRouteManagers::new();
        let listener_a = make_listener_key(8443, "TLS");
        let listener_b = make_listener_key(8444, "TLS");

        // Pre-seed B's manager with a prior route.
        let mgr_b = managers.get_or_create_listener_manager(&listener_b);
        let mut prior: HashMap<String, Arc<TlsRouteUnit>> = HashMap::new();
        prior.insert(
            "default/prior".to_string(),
            test_unit("default", "prior", listener_b.clone()),
        );
        mgr_b.rebuild(&prior);
        assert!(
            mgr_b.load_route_table().has_catch_all(),
            "precondition: B has a non-empty table"
        );

        // route_cache only references A.
        managers.insert_route(test_unit("default", "route-a", listener_a.clone()));

        drive_rebuild_all(&managers, vec![listener_a.clone(), listener_b.clone()]);

        assert!(
            !mgr_b.load_route_table().has_catch_all(),
            "active-but-no-routes TLS listener should be cleared"
        );
        assert!(managers.by_listener.contains_key(&listener_a));
        assert!(managers.by_listener.contains_key(&listener_b));
    }

    #[test]
    fn test_rebuild_all_removes_stale_inactive_listener() {
        let managers = GlobalTlsRouteManagers::new();
        let stale = make_listener_key(8445, "TLS");
        managers.get_or_create_listener_manager(&stale);
        assert!(managers.by_listener.contains_key(&stale));

        drive_rebuild_all(&managers, vec![]);

        assert!(
            !managers.by_listener.contains_key(&stale),
            "inactive-and-no-routes TLS by_listener entry should be removed"
        );
    }

    #[test]
    fn test_rebuild_affected_clears_listener_with_empty_bucket() {
        let managers = GlobalTlsRouteManagers::new();
        let listener = make_listener_key(8446, "TLS");

        let mgr = managers.get_or_create_listener_manager(&listener);
        let mut prior: HashMap<String, Arc<TlsRouteUnit>> = HashMap::new();
        prior.insert(
            "default/prior".to_string(),
            test_unit("default", "prior", listener.clone()),
        );
        mgr.rebuild(&prior);
        assert!(
            mgr.load_route_table().has_catch_all(),
            "precondition: listener has the catch-all prior route"
        );

        let mut affected: HashSet<ListenerKey> = HashSet::new();
        affected.insert(listener.clone());
        drive_rebuild_affected(&managers, &affected, vec![listener.clone()]);

        assert!(
            !mgr.load_route_table().has_catch_all(),
            "affected TLS listener with no routes should be cleared"
        );
        assert!(managers.by_listener.contains_key(&listener));
    }

    #[test]
    fn test_rebuild_affected_orphan_sweep() {
        let managers = GlobalTlsRouteManagers::new();
        let affected_key = make_listener_key(8447, "TLS");
        let orphan_key = make_listener_key(8448, "TLS");

        managers.get_or_create_listener_manager(&orphan_key);
        managers.insert_route(test_unit("default", "route-x", affected_key.clone()));

        let mut affected: HashSet<ListenerKey> = HashSet::new();
        affected.insert(affected_key.clone());
        drive_rebuild_affected(&managers, &affected, vec![affected_key.clone()]);

        assert!(!managers.by_listener.contains_key(&orphan_key));
        assert!(managers.by_listener.contains_key(&affected_key));
    }
}

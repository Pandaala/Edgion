use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::tcp::tcp_route_table::TcpRouteTable;
use crate::types::resources::TCPRoute;
use crate::types::ListenerKey;
use crate::types::ResourceMeta;

/// Per-listener TCP route manager.
///
/// Each instance owns an atomically-swappable `TcpRouteTable` snapshot.
/// `EdgionTcpProxy` holds an `Arc<TcpPortRouteManager>` and calls
/// `load_route_table()` per-connection for lock-free lookups.
///
/// Route data is owned by `GlobalTcpRouteManagers`; this struct only
/// holds the compiled route snapshot for one listener.
pub struct TcpPortRouteManager {
    route_table: ArcSwap<TcpRouteTable>,
}

impl Default for TcpPortRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpPortRouteManager {
    pub fn new() -> Self {
        Self {
            route_table: ArcSwap::from_pointee(TcpRouteTable::new()),
        }
    }

    /// Load the current route table snapshot (hot path, per-connection).
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<TcpRouteTable>> {
        self.route_table.load()
    }

    /// Rebuild the route table from routes belonging to this listener.
    /// Called by `GlobalTcpRouteManagers` during rebuild.
    pub fn rebuild(&self, routes: &HashMap<String, Arc<TCPRoute>>) {
        let new_table = TcpRouteTable::from_routes(routes);

        tracing::debug!(
            component = "tcp_port_route_manager",
            routes = routes.len(),
            "Rebuilt per-listener TCP route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `ListenerKey -> Arc<TcpPortRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> Arc<TCPRoute>`) and
/// implements `ConfHandler<TCPRoute>`. On every change it rebuilds all
/// per-listener managers by resolving each route's target listeners from its
/// `spec.resolved_listeners`.
pub struct GlobalTcpRouteManagers {
    /// Canonical route store: resource_key -> initialized Arc<TCPRoute>
    route_cache: DashMap<String, Arc<TCPRoute>>,

    /// listener -> per-listener manager (stable Arc held by listeners)
    by_listener: DashMap<ListenerKey, Arc<TcpPortRouteManager>>,

    /// Serializes the read-snapshot-then-rewrite sequences that mutate
    /// `route_cache` + `by_listener`. The TCPRoute cache task
    /// (`full_set`/`partial_update`) and the Gateway cache task
    /// (`rebuild_all_listener_managers` via `runtime::handler`) reach this manager
    /// concurrently under different per-kind locks; this is the only lock both
    /// contend on, preventing a stale-snapshot rebuild leak. Synchronous only.
    rebuild_lock: parking_lot::Mutex<()>,
}

impl Default for GlobalTcpRouteManagers {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalTcpRouteManagers {
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

    /// Get or create a per-listener `TcpPortRouteManager`.
    ///
    /// Called by `listener_builder` at startup. The returned `Arc` is stable;
    /// route updates only swap the inner `ArcSwap<TcpRouteTable>`.
    pub fn get_or_create_listener_manager(&self, key: &ListenerKey) -> Arc<TcpPortRouteManager> {
        self.by_listener
            .entry(key.clone())
            .or_insert_with(|| Arc::new(TcpPortRouteManager::new()))
            .value()
            .clone()
    }

    /// Insert an initialized route into the cache.
    pub fn insert_route(&self, route: Arc<TCPRoute>) {
        let key = route.key_name();
        self.route_cache.insert(key, route);
    }

    /// Get a route from the cache by key.
    pub fn get_route(&self, key: &str) -> Option<Arc<TCPRoute>> {
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
    /// `routes::l4_rebuild::rebuild_all_l4_listener_managers`, which (a) buckets
    /// each route by its target listeners, (b) rebuilds every listener that has
    /// routes, (c) drops `by_listener` entries that are stale in PGIS, and (d)
    /// clears the snapshot of any active listener that no longer has routes so
    /// the data plane stops forwarding to deleted backends.
    pub fn rebuild_all_listener_managers(&self) {
        let _guard = self.rebuild_lock.lock();
        self.rebuild_all_listener_managers_locked();
    }

    /// Rebuild body. The caller MUST hold `rebuild_lock`.
    pub(crate) fn rebuild_all_listener_managers_locked(&self) {
        let active = crate::core::gateway::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::core::gateway::routes::l4_rebuild::rebuild_all_l4_listener_managers(
            "global_tcp_route_managers",
            &self.by_listener,
            &self.route_cache,
            active,
            |r| resolved_listeners_for_route(r).to_vec(),
            |k| self.get_or_create_listener_manager(k),
            |m, routes| m.rebuild(routes),
        );
    }

    /// Rebuild only the per-listener managers for the given set of affected
    /// listeners. Delegates to the shared L4 orchestrator
    /// `routes::l4_rebuild::rebuild_affected_l4_listener_managers`.
    pub fn rebuild_affected_listener_managers(&self, affected_listeners: &HashSet<ListenerKey>) {
        let active = crate::core::gateway::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::core::gateway::routes::l4_rebuild::rebuild_affected_l4_listener_managers(
            "global_tcp_route_managers",
            &self.by_listener,
            &self.route_cache,
            affected_listeners,
            active,
            |r| resolved_listeners_for_route(r).to_vec(),
            |k| self.get_or_create_listener_manager(k),
            |m, routes| m.rebuild(routes),
        );
    }
}

/// Get the resolved listeners for a TCPRoute.
///
/// Uses `spec.resolved_listeners` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.
pub(crate) fn resolved_listeners_for_route(route: &TCPRoute) -> &[ListenerKey] {
    route.spec.resolved_listeners.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TcpRouteManagerStats {
    pub route_cache: usize,
    pub listener_count: usize,
}

impl GlobalTcpRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> TcpRouteManagerStats {
        TcpRouteManagerStats {
            route_cache: self.route_cache.len(),
            listener_count: self.by_listener.len(),
        }
    }
}

static GLOBAL_TCP_ROUTE_MANAGERS: OnceLock<GlobalTcpRouteManagers> = OnceLock::new();

pub fn get_global_tcp_route_managers() -> &'static GlobalTcpRouteManagers {
    GLOBAL_TCP_ROUTE_MANAGERS.get_or_init(GlobalTcpRouteManagers::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::routes::l4_rebuild::{
        rebuild_affected_l4_listener_managers, rebuild_all_l4_listener_managers,
    };
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::tcp_route::TCPRouteSpec;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_tcp_route(namespace: &str, name: &str, listener_key: ListenerKey) -> TCPRoute {
        TCPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TCPRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: Some("tcp".to_string()),
                    port: None,
                }]),
                rules: Some(vec![]),
                resolved_listeners: Some(vec![listener_key]),
            },
            status: None,
        }
    }

    /// Drive the shared helper directly with a synthetic `active` PGIS snapshot
    /// so tests do not race against the global `ListenerGatewayInfoStore`.
    fn drive_rebuild_all(managers: &GlobalTcpRouteManagers, active: Vec<ListenerKey>) {
        rebuild_all_l4_listener_managers(
            "global_tcp_route_managers",
            &managers.by_listener,
            &managers.route_cache,
            active,
            |r| resolved_listeners_for_route(r).to_vec(),
            |k| managers.get_or_create_listener_manager(k),
            |m, routes| m.rebuild(routes),
        );
    }

    fn drive_rebuild_affected(
        managers: &GlobalTcpRouteManagers,
        affected: &HashSet<ListenerKey>,
        active: Vec<ListenerKey>,
    ) {
        rebuild_affected_l4_listener_managers(
            "global_tcp_route_managers",
            &managers.by_listener,
            &managers.route_cache,
            affected,
            active,
            |r| resolved_listeners_for_route(r).to_vec(),
            |k| managers.get_or_create_listener_manager(k),
            |m, routes| m.rebuild(routes),
        );
    }

    /// Regression for F-007: `rebuild_all` must clear the route table of an
    /// active listener that previously had routes but now has none. Without
    /// the clear-pass, the listener's `ArcSwap<TcpRouteTable>` retains the
    /// previous snapshot and the data plane keeps forwarding to deleted
    /// backends.
    #[test]
    fn test_rebuild_all_clears_active_listener_with_no_routes() {
        let managers = GlobalTcpRouteManagers::new();
        let listener_a = make_listener_key(9000, "TCP");
        let listener_b = make_listener_key(9001, "TCP");

        // Seed B's manager with a prior route to mimic state from an earlier
        // rebuild_all (the by_listener entry pre-exists).
        let mgr_b = managers.get_or_create_listener_manager(&listener_b);
        let prior_route = Arc::new(create_test_tcp_route("default", "prior", listener_b.clone()));
        let mut prior: HashMap<String, Arc<TCPRoute>> = HashMap::new();
        prior.insert("default/prior".to_string(), prior_route);
        mgr_b.rebuild(&prior);
        assert_eq!(mgr_b.load_route_table().route_count(), 1, "precondition: B has 1 route");

        // Current route_cache only references listener A.
        let route_a = Arc::new(create_test_tcp_route("default", "route-a", listener_a.clone()));
        managers.insert_route(route_a);

        // Both A and B are still active in PGIS.
        drive_rebuild_all(&managers, vec![listener_a.clone(), listener_b.clone()]);

        // B's snapshot must be cleared, not stale.
        assert_eq!(
            mgr_b.load_route_table().route_count(),
            0,
            "active-but-no-routes listener should be cleared"
        );
        // A should hold its current route.
        let mgr_a = managers
            .by_listener
            .get(&listener_a)
            .map(|e| e.value().clone())
            .unwrap();
        assert_eq!(mgr_a.load_route_table().route_count(), 1, "A retains its route");
        // Neither is removed from by_listener (both still active).
        assert!(managers.by_listener.contains_key(&listener_a));
        assert!(managers.by_listener.contains_key(&listener_b));
    }

    /// `rebuild_all` drops `by_listener` entries that have no routes AND are
    /// no longer active in PGIS.
    #[test]
    fn test_rebuild_all_removes_stale_inactive_listener() {
        let managers = GlobalTcpRouteManagers::new();
        let stale = make_listener_key(9100, "TCP");

        // Pre-populate stale's manager (e.g. left over from a previous rebuild).
        managers.get_or_create_listener_manager(&stale);
        assert!(managers.by_listener.contains_key(&stale));

        // No routes, no active listeners.
        drive_rebuild_all(&managers, vec![]);

        assert!(
            !managers.by_listener.contains_key(&stale),
            "by_listener entry with no routes and not in PGIS should be removed"
        );
    }

    /// `rebuild_affected` rebuilds only the listeners passed in `affected`,
    /// and on an affected listener that has no routes, clears its snapshot.
    #[test]
    fn test_rebuild_affected_clears_listener_with_empty_bucket() {
        let managers = GlobalTcpRouteManagers::new();
        let listener = make_listener_key(9200, "TCP");

        // Pre-seed the listener with a route so the route_table is non-empty.
        let mgr = managers.get_or_create_listener_manager(&listener);
        let prior_route = Arc::new(create_test_tcp_route("default", "prior", listener.clone()));
        let mut prior: HashMap<String, Arc<TCPRoute>> = HashMap::new();
        prior.insert("default/prior".to_string(), prior_route);
        mgr.rebuild(&prior);
        assert_eq!(mgr.load_route_table().route_count(), 1);

        // route_cache is empty (all routes for this listener were removed); the
        // listener is still active in PGIS and is in `affected`.
        let mut affected: HashSet<ListenerKey> = HashSet::new();
        affected.insert(listener.clone());
        drive_rebuild_affected(&managers, &affected, vec![listener.clone()]);

        assert_eq!(
            mgr.load_route_table().route_count(),
            0,
            "affected listener with no routes should be cleared"
        );
        assert!(
            managers.by_listener.contains_key(&listener),
            "active listener stays in by_listener even when cleared"
        );
    }

    /// `rebuild_affected` orphan-sweep removes by_listener entries that are
    /// not referenced by any current route and no longer active in PGIS.
    #[test]
    fn test_rebuild_affected_orphan_sweep() {
        let managers = GlobalTcpRouteManagers::new();
        let affected_key = make_listener_key(9300, "TCP");
        let orphan_key = make_listener_key(9301, "TCP");

        // Pre-create an orphan by_listener entry (no routes reference it, and
        // it is not in the active set).
        managers.get_or_create_listener_manager(&orphan_key);
        // And the affected listener which currently has a route.
        let route = Arc::new(create_test_tcp_route("default", "route-x", affected_key.clone()));
        managers.insert_route(route);

        let mut affected: HashSet<ListenerKey> = HashSet::new();
        affected.insert(affected_key.clone());
        drive_rebuild_affected(&managers, &affected, vec![affected_key.clone()]);

        assert!(
            !managers.by_listener.contains_key(&orphan_key),
            "orphan by_listener entry should be swept"
        );
        assert!(managers.by_listener.contains_key(&affected_key));
    }
}

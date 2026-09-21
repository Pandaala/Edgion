use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::routes::tcp::route_unit::TcpRouteUnit;
use crate::routes::tcp::tcp_route_table::TcpRouteTable;
use edgion_resources::resources::TCPRoute;
use edgion_resources::ListenerKey;

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

    /// Rebuild the route table from units belonging to this listener.
    /// Called by `GlobalTcpRouteManagers` during rebuild.
    pub fn rebuild(&self, units: &HashMap<String, Arc<TcpRouteUnit>>) {
        let new_table = TcpRouteTable::from_units(units);

        tracing::debug!(
            component = "tcp_port_route_manager",
            routes = units.len(),
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
    /// Canonical route store: resource_key -> compiled Arc<TcpRouteUnit>.
    ///
    /// Units (and their `backend_finder` WRR cursor) are built ONCE in the
    /// conf-handler and reused across listener rebuilds, so an unrelated route
    /// change on the same listener does not reset a sibling route's selector.
    route_cache: DashMap<String, Arc<TcpRouteUnit>>,

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

    /// Insert a compiled route unit into the cache.
    pub fn insert_route(&self, unit: Arc<TcpRouteUnit>) {
        let key = unit.key_name();
        self.route_cache.insert(key, unit);
    }

    /// Get a route unit from the cache by key.
    pub fn get_route(&self, key: &str) -> Option<Arc<TcpRouteUnit>> {
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
        let active = crate::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::routes::l4_rebuild::rebuild_all_l4_listener_managers(
            "global_tcp_route_managers",
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
        let active = crate::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::routes::l4_rebuild::rebuild_affected_l4_listener_managers(
            "global_tcp_route_managers",
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
    /// Sum of per-listener route-table sizes across `by_listener`. Mirrors
    /// HTTP's `exact_domains` so orphan residue in any per-listener table is
    /// observable via stats even when `route_cache` is empty.
    pub total_routes_in_tables: usize,
}

impl GlobalTcpRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> TcpRouteManagerStats {
        let mut total_routes_in_tables = 0usize;
        for entry in self.by_listener.iter() {
            total_routes_in_tables += entry.value().load_route_table().route_count();
        }
        TcpRouteManagerStats {
            route_cache: self.route_cache.len(),
            listener_count: self.by_listener.len(),
            total_routes_in_tables,
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
    use crate::routes::l4_rebuild::{rebuild_affected_l4_listener_managers, rebuild_all_l4_listener_managers};
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::resources::tcp_route::TCPRouteSpec;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_tcp_route(namespace: &str, name: &str, listener_key: ListenerKey) -> TCPRoute {
        crate::test_current_status::processed(TCPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TCPRouteSpec {
                current_status: None,
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
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
        })
    }

    fn test_unit(namespace: &str, name: &str, listener_key: ListenerKey) -> Arc<TcpRouteUnit> {
        Arc::new(TcpRouteUnit::from_route(Arc::new(create_test_tcp_route(
            namespace,
            name,
            listener_key,
        ))))
    }

    /// Drive the shared helper directly with a synthetic `active` PGIS snapshot
    /// so tests do not race against the global `ListenerGatewayInfoStore`.
    fn drive_rebuild_all(managers: &GlobalTcpRouteManagers, active: Vec<ListenerKey>) {
        rebuild_all_l4_listener_managers(
            "global_tcp_route_managers",
            &managers.by_listener,
            &managers.route_cache,
            active,
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |k| managers.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
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
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |k| managers.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
        );
    }

    /// Invariant: `rebuild_all` must clear the route table of an active listener
    /// that previously had routes but now has none. Without the clear-pass, the
    /// listener's `ArcSwap<TcpRouteTable>` retains the previous snapshot and the
    /// data plane keeps forwarding to deleted backends.
    #[test]
    fn test_rebuild_all_clears_active_listener_with_no_routes() {
        let managers = GlobalTcpRouteManagers::new();
        let listener_a = make_listener_key(9000, "TCP");
        let listener_b = make_listener_key(9001, "TCP");

        // Seed B's manager with a prior route to mimic state from an earlier
        // rebuild_all (the by_listener entry pre-exists).
        let mgr_b = managers.get_or_create_listener_manager(&listener_b);
        let mut prior: HashMap<String, Arc<TcpRouteUnit>> = HashMap::new();
        prior.insert(
            "default/prior".to_string(),
            test_unit("default", "prior", listener_b.clone()),
        );
        mgr_b.rebuild(&prior);
        assert_eq!(mgr_b.load_route_table().route_count(), 1, "precondition: B has 1 route");

        // Current route_cache only references listener A.
        managers.insert_route(test_unit("default", "route-a", listener_a.clone()));

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

        // Retain the same manager Arc a live proxy would hold and seed a real route.
        let retained = managers.get_or_create_listener_manager(&stale);
        let mut prior = HashMap::new();
        prior.insert(
            "default/prior".to_string(),
            test_unit("default", "prior", stale.clone()),
        );
        retained.rebuild(&prior);
        assert_eq!(retained.load_route_table().route_count(), 1);
        assert!(managers.by_listener.contains_key(&stale));

        // No routes, no active listeners.
        drive_rebuild_all(&managers, vec![]);

        assert!(
            !managers.by_listener.contains_key(&stale),
            "by_listener entry with no routes and not in PGIS should be removed"
        );
        assert_eq!(
            retained.load_route_table().route_count(),
            0,
            "a retained handle must be cleared when its registry entry is removed"
        );
        let recreated = managers.get_or_create_listener_manager(&stale);
        assert!(!Arc::ptr_eq(&retained, &recreated));
    }

    /// `rebuild_affected` rebuilds only the listeners passed in `affected`,
    /// and on an affected listener that has no routes, clears its snapshot.
    #[test]
    fn test_rebuild_affected_clears_listener_with_empty_bucket() {
        let managers = GlobalTcpRouteManagers::new();
        let listener = make_listener_key(9200, "TCP");

        // Pre-seed the listener with a route so the route_table is non-empty.
        let mgr = managers.get_or_create_listener_manager(&listener);
        let mut prior: HashMap<String, Arc<TcpRouteUnit>> = HashMap::new();
        prior.insert(
            "default/prior".to_string(),
            test_unit("default", "prior", listener.clone()),
        );
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

        // Retain and seed the orphan manager as a live proxy could have done
        // before its listener disappeared from PGIS.
        let retained_orphan = managers.get_or_create_listener_manager(&orphan_key);
        let mut prior = HashMap::new();
        prior.insert(
            "default/orphan-prior".to_string(),
            test_unit("default", "orphan-prior", orphan_key.clone()),
        );
        retained_orphan.rebuild(&prior);
        assert_eq!(retained_orphan.load_route_table().route_count(), 1);
        // And the affected listener which currently has a route.
        managers.insert_route(test_unit("default", "route-x", affected_key.clone()));

        let mut affected: HashSet<ListenerKey> = HashSet::new();
        affected.insert(affected_key.clone());
        drive_rebuild_affected(&managers, &affected, vec![affected_key.clone()]);

        assert!(
            !managers.by_listener.contains_key(&orphan_key),
            "orphan by_listener entry should be swept"
        );
        assert_eq!(
            retained_orphan.load_route_table().route_count(),
            0,
            "the orphan sweep must clear a retained manager handle"
        );
        let recreated = managers.get_or_create_listener_manager(&orphan_key);
        assert!(!Arc::ptr_eq(&retained_orphan, &recreated));
        assert!(managers.by_listener.contains_key(&affected_key));
    }

    /// Invariant: a listener-table rebuild must Arc-CLONE the cached unit, never
    /// rebuild it. If the served unit is the same `Arc` as the cached one, its
    /// `backend_finder` (and WRR cursor) is preserved across rebuilds, so an
    /// unrelated route/listener change does not reset a route's load-balancing
    /// distribution. A reintroduction of "build the unit inside `from_units`"
    /// would make `ptr_eq` fail here.
    #[test]
    fn test_unit_arc_reused_across_table_rebuild() {
        let managers = GlobalTcpRouteManagers::new();
        let key = make_listener_key(9700, "TCP");
        managers.insert_route(test_unit("default", "route1", key.clone()));
        let cached = managers.get_route("default/route1").unwrap();

        // Gateway-driven rebuild of the listener's table.
        drive_rebuild_all(&managers, vec![key.clone()]);
        let served = managers
            .by_listener
            .get(&key)
            .unwrap()
            .load_route_table()
            .match_route()
            .unwrap();
        assert!(
            Arc::ptr_eq(&cached, &served),
            "table must serve the cached unit Arc, not a freshly-built one"
        );

        // A second rebuild must still serve the very same Arc.
        drive_rebuild_all(&managers, vec![key.clone()]);
        let served2 = managers
            .by_listener
            .get(&key)
            .unwrap()
            .load_route_table()
            .match_route()
            .unwrap();
        assert!(
            Arc::ptr_eq(&cached, &served2),
            "unit Arc must survive repeated rebuilds"
        );
    }
}

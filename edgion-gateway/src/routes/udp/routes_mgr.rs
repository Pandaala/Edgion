use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::routes::udp::route_unit::UdpRouteUnit;
use crate::routes::udp::udp_route_table::UdpRouteTable;
use edgion_resources::resources::UDPRoute;
use edgion_resources::ListenerKey;

/// Per-listener UDP route manager.
///
/// Each instance owns an atomically-swappable `UdpRouteTable` snapshot.
/// `EdgionUdpProxy` holds an `Arc<UdpPortRouteManager>` and calls
/// `load_route_table()` when routing a new client session. Established sessions
/// use their pinned upstream without another table lookup.
///
/// Route data is owned by `GlobalUdpRouteManagers`; this struct only
/// holds the compiled route snapshot for one listener.
pub struct UdpPortRouteManager {
    route_table: ArcSwap<UdpRouteTable>,
}

impl Default for UdpPortRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UdpPortRouteManager {
    pub fn new() -> Self {
        Self {
            route_table: ArcSwap::from_pointee(UdpRouteTable::new()),
        }
    }

    /// Load the current route table snapshot when routing a new client session.
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<UdpRouteTable>> {
        self.route_table.load()
    }

    /// Rebuild the route table from units belonging to this listener.
    /// Called by `GlobalUdpRouteManagers` during rebuild.
    pub fn rebuild(&self, units: &HashMap<String, Arc<UdpRouteUnit>>) {
        let new_table = UdpRouteTable::from_units(units);

        tracing::debug!(
            component = "udp_port_route_manager",
            routes = units.len(),
            "Rebuilt per-listener UDP route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `ListenerKey -> Arc<UdpPortRouteManager>`.
///
/// Owns the canonical compiled-unit cache
/// (`resource_key -> Arc<UdpRouteUnit>`) used by `ConfHandler<UDPRoute>`.
/// Full updates rebuild every listener table; partial updates publish tables
/// only for affected listeners, using each unit's controller-resolved listener
/// keys.
pub struct GlobalUdpRouteManagers {
    /// Canonical route store: resource_key -> compiled Arc<UdpRouteUnit>.
    ///
    /// Units (and their `backend_finder` WRR cursor) are built ONCE in the
    /// conf-handler and reused across listener rebuilds, so an unrelated route
    /// change on the same listener does not reset a sibling route's selector.
    route_cache: DashMap<String, Arc<UdpRouteUnit>>,

    /// listener -> per-listener manager (stable Arc held by listeners)
    by_listener: DashMap<ListenerKey, Arc<UdpPortRouteManager>>,

    /// Serializes the read-snapshot-then-rewrite sequences that mutate
    /// `route_cache` + `by_listener`. The UDPRoute cache task
    /// (`full_set`/`partial_update`) and the Gateway cache task
    /// (`rebuild_all_listener_managers` via `runtime::handler`) reach this manager
    /// concurrently under different per-kind locks; this is the only lock both
    /// contend on, preventing a stale-snapshot rebuild leak. Synchronous only.
    rebuild_lock: parking_lot::Mutex<()>,
}

impl Default for GlobalUdpRouteManagers {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalUdpRouteManagers {
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

    /// Get or create a per-listener `UdpPortRouteManager`.
    ///
    /// Called by `listener_builder` at startup. The returned `Arc` is stable;
    /// route updates only swap the inner `ArcSwap<UdpRouteTable>`.
    pub fn get_or_create_listener_manager(&self, key: &ListenerKey) -> Arc<UdpPortRouteManager> {
        self.by_listener
            .entry(key.clone())
            .or_insert_with(|| Arc::new(UdpPortRouteManager::new()))
            .value()
            .clone()
    }

    /// Insert a compiled route unit into the cache.
    pub fn insert_route(&self, unit: Arc<UdpRouteUnit>) {
        let key = unit.key_name();
        self.route_cache.insert(key, unit);
    }

    /// Get a route unit from the cache by key.
    pub fn get_route(&self, key: &str) -> Option<Arc<UdpRouteUnit>> {
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
        let active = crate::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::routes::l4_rebuild::rebuild_all_l4_listener_managers(
            "global_udp_route_managers",
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
            "global_udp_route_managers",
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

/// Get the resolved listeners for a UDPRoute.
///
/// Uses `spec.resolved_listeners` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.
pub(crate) fn resolved_listeners_for_route(route: &UDPRoute) -> &[ListenerKey] {
    route.spec.resolved_listeners.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UdpRouteManagerStats {
    pub route_cache: usize,
    pub listener_count: usize,
    /// Sum of route entries across every current listener-table snapshot.
    /// A UDPRoute attached to multiple listeners contributes once per table,
    /// so this is deliberately not a unique resource count.
    pub total_routes_in_tables: usize,
}

impl GlobalUdpRouteManagers {
    /// Collect cold-path size statistics for leak-detection tests.
    ///
    /// Each listener's ArcSwap snapshot is loaded independently without the
    /// rebuild lock, so this does not claim cross-manager atomicity.
    pub fn stats(&self) -> UdpRouteManagerStats {
        let mut total_routes_in_tables = 0usize;
        for entry in self.by_listener.iter() {
            total_routes_in_tables += entry.value().load_route_table().route_count();
        }
        UdpRouteManagerStats {
            route_cache: self.route_cache.len(),
            listener_count: self.by_listener.len(),
            total_routes_in_tables,
        }
    }
}

static GLOBAL_UDP_ROUTE_MANAGERS: OnceLock<GlobalUdpRouteManagers> = OnceLock::new();

pub fn get_global_udp_route_managers() -> &'static GlobalUdpRouteManagers {
    GLOBAL_UDP_ROUTE_MANAGERS.get_or_init(GlobalUdpRouteManagers::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::udp_route::*;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_udp_route(namespace: &str, name: &str, listener_key: ListenerKey) -> UDPRoute {
        use edgion_resources::resources::common::ParentReference;
        UDPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: UDPRouteSpec {
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: Some("udp".to_string()),
                    port: None,
                }]),
                rules: Some(vec![]),
                resolved_listeners: Some(vec![listener_key]),
            },
            status: None,
        }
    }

    fn test_unit(namespace: &str, name: &str, listener_key: ListenerKey) -> Arc<UdpRouteUnit> {
        Arc::new(UdpRouteUnit::from_route(Arc::new(create_test_udp_route(
            namespace,
            name,
            listener_key,
        ))))
    }

    #[test]
    fn test_get_or_create_listener_manager() {
        let managers = GlobalUdpRouteManagers::new();
        let key1 = make_listener_key(9000, "UDP");
        let mgr1 = managers.get_or_create_listener_manager(&key1);
        let mgr2 = managers.get_or_create_listener_manager(&key1);
        assert!(Arc::ptr_eq(&mgr1, &mgr2));

        let key2 = make_listener_key(9001, "UDP");
        let mgr3 = managers.get_or_create_listener_manager(&key2);
        assert!(!Arc::ptr_eq(&mgr1, &mgr3));
    }

    #[test]
    fn test_insert_and_get_route() {
        let managers = GlobalUdpRouteManagers::new();
        let key = make_listener_key(9000, "UDP");
        let unit = test_unit("default", "route1", key);
        let route_key = unit.key_name();

        managers.insert_route(unit.clone());
        let found = managers.get_route(&route_key);
        assert!(found.is_some());

        managers.remove_route(&route_key);
        let found = managers.get_route(&route_key);
        assert!(found.is_none());
    }

    #[test]
    fn stats_report_empty_and_retained_table_routes() {
        let managers = GlobalUdpRouteManagers::new();
        let empty = managers.stats();
        assert_eq!(empty.route_cache, 0);
        assert_eq!(empty.listener_count, 0);
        assert_eq!(empty.total_routes_in_tables, 0);

        let listener_a = make_listener_key(9000, "UDP");
        let listener_b = make_listener_key(9001, "UDP");
        let unit = test_unit("default", "route1", listener_a.clone());
        let routes = HashMap::from([(unit.key_name(), unit)]);
        managers.get_or_create_listener_manager(&listener_a).rebuild(&routes);
        managers.get_or_create_listener_manager(&listener_b).rebuild(&routes);

        let stats = managers.stats();
        assert_eq!(stats.route_cache, 0, "listener tables retain their own snapshots");
        assert_eq!(stats.listener_count, 2);
        assert_eq!(
            stats.total_routes_in_tables, 2,
            "entries are counted per listener table"
        );
    }

    #[test]
    fn test_clear_route_cache() {
        let managers = GlobalUdpRouteManagers::new();
        let key = make_listener_key(9000, "UDP");
        managers.insert_route(test_unit("default", "route1", key));

        assert_eq!(managers.stats().route_cache, 1);
        managers.clear_route_cache();
        assert_eq!(managers.stats().route_cache, 0);
    }

    /// Drive the shared helper directly with a synthetic `active` PGIS
    /// snapshot so tests do not race against the global store.
    fn drive_rebuild_all(managers: &GlobalUdpRouteManagers, active: Vec<ListenerKey>) {
        crate::routes::l4_rebuild::rebuild_all_l4_listener_managers(
            "global_udp_route_managers",
            &managers.by_listener,
            &managers.route_cache,
            active,
            |u| resolved_listeners_for_route(&u.route).to_vec(),
            |k| managers.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
        );
    }

    fn drive_rebuild_affected(
        managers: &GlobalUdpRouteManagers,
        affected: &HashSet<ListenerKey>,
        active: Vec<ListenerKey>,
    ) {
        crate::routes::l4_rebuild::rebuild_affected_l4_listener_managers(
            "global_udp_route_managers",
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
    /// that previously had routes but now has none.
    #[test]
    fn test_rebuild_all_clears_active_listener_with_no_routes() {
        let managers = GlobalUdpRouteManagers::new();
        let listener_a = make_listener_key(9500, "UDP");
        let listener_b = make_listener_key(9501, "UDP");

        let mgr_b = managers.get_or_create_listener_manager(&listener_b);
        let mut prior: HashMap<String, Arc<UdpRouteUnit>> = HashMap::new();
        prior.insert(
            "default/prior".to_string(),
            test_unit("default", "prior", listener_b.clone()),
        );
        mgr_b.rebuild(&prior);
        assert_eq!(mgr_b.load_route_table().route_count(), 1, "precondition: B has 1 route");

        managers.insert_route(test_unit("default", "route-a", listener_a.clone()));

        drive_rebuild_all(&managers, vec![listener_a.clone(), listener_b.clone()]);

        assert_eq!(
            mgr_b.load_route_table().route_count(),
            0,
            "active-but-no-routes UDP listener should be cleared"
        );
        let mgr_a = managers
            .by_listener
            .get(&listener_a)
            .map(|e| e.value().clone())
            .unwrap();
        assert_eq!(mgr_a.load_route_table().route_count(), 1);
        assert!(managers.by_listener.contains_key(&listener_a));
        assert!(managers.by_listener.contains_key(&listener_b));
    }

    #[test]
    fn test_rebuild_affected_clears_listener_with_empty_bucket() {
        let managers = GlobalUdpRouteManagers::new();
        let listener = make_listener_key(9600, "UDP");

        let mgr = managers.get_or_create_listener_manager(&listener);
        let mut prior: HashMap<String, Arc<UdpRouteUnit>> = HashMap::new();
        prior.insert(
            "default/prior".to_string(),
            test_unit("default", "prior", listener.clone()),
        );
        mgr.rebuild(&prior);
        assert_eq!(mgr.load_route_table().route_count(), 1);

        let mut affected: HashSet<ListenerKey> = HashSet::new();
        affected.insert(listener.clone());
        drive_rebuild_affected(&managers, &affected, vec![listener.clone()]);

        assert_eq!(mgr.load_route_table().route_count(), 0);
        assert!(managers.by_listener.contains_key(&listener));
        let cleared = managers.stats();
        assert_eq!(cleared.route_cache, 0);
        assert_eq!(cleared.listener_count, 1);
        assert_eq!(cleared.total_routes_in_tables, 0);
    }
}

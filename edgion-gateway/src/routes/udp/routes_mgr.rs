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
            |u| resolved_listeners_for_route(u.route()).to_vec(),
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
            |u| resolved_listeners_for_route(u.route()).to_vec(),
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
#[path = "../../../tests/unit/routes/udp/routes_mgr_test.rs"]
mod tests;

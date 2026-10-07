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
            "global_tcp_route_managers",
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
#[path = "../../../tests/unit/routes/tcp/routes_mgr_test.rs"]
mod tests;

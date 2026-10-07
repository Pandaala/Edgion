use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::routes::tls::gateway_tls_routes::TlsRouteTable;
use crate::routes::tls::route_unit::TlsRouteUnit;
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
    listener_key: ListenerKey,
    route_table: ArcSwap<TlsRouteTable>,
}

impl TlsRouteManager {
    pub fn new(listener_key: ListenerKey) -> Self {
        Self {
            listener_key,
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
        let previous = self.route_table.load();
        let new_table = previous.with_units(&self.listener_key, units);

        tracing::debug!(
            component = "tls_route_manager",
            routes = units.len(),
            "Rebuilt per-listener TLS route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `ListenerKey -> Arc<TlsRouteManager>`.
///
/// Owns the canonical compiled-unit cache
/// (`resource_key -> Arc<TlsRouteUnit>`) used by `ConfHandler<TLSRoute>`.
/// Full updates rebuild every listener table; partial updates publish tables
/// only for affected listeners, using each unit's controller-resolved listener
/// keys.
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
            .or_insert_with(|| Arc::new(TlsRouteManager::new(key.clone())))
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
        let active = crate::runtime::store::get_listener_gateway_info_store().all_listener_keys();
        crate::routes::l4_rebuild::rebuild_all_l4_listener_managers(
            "global_tls_route_managers",
            &self.by_listener,
            &self.route_cache,
            active,
            |u| resolved_listeners_for_route(u.route()),
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
            "global_tls_route_managers",
            &self.by_listener,
            &self.route_cache,
            affected_listeners,
            active,
            |u| resolved_listeners_for_route(u.route()),
            |k| self.get_or_create_listener_manager(k),
            |m, units| m.rebuild(units),
        );
    }
}

/// Get the resolved listeners for a TLSRoute.
///
/// Uses `spec.resolved_listeners` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.
pub(crate) fn resolved_listeners_for_route(route: &TLSRoute) -> Vec<ListenerKey> {
    let mut keys: Vec<_> = route
        .spec
        .resolved_listeners
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|attachment| attachment.listener_key.clone())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TlsRouteManagerStats {
    pub route_cache: usize,
    pub listener_count: usize,
    /// Sum of retained candidate entries across every current listener-table
    /// snapshot. A Route can contribute more than once across hostnames and
    /// listeners, so this is deliberately not a unique resource count.
    pub total_candidate_entries_in_tables: usize,
}

impl GlobalTlsRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> TlsRouteManagerStats {
        let mut total_candidate_entries_in_tables = 0usize;
        for entry in self.by_listener.iter() {
            total_candidate_entries_in_tables += entry.value().load_route_table().candidate_entry_count();
        }
        TlsRouteManagerStats {
            route_cache: self.route_cache.len(),
            listener_count: self.by_listener.len(),
            total_candidate_entries_in_tables,
        }
    }
}

static GLOBAL_TLS_ROUTE_MANAGERS: OnceLock<GlobalTlsRouteManagers> = OnceLock::new();

pub fn get_global_tls_route_managers() -> &'static GlobalTlsRouteManagers {
    GLOBAL_TLS_ROUTE_MANAGERS.get_or_init(GlobalTlsRouteManagers::new)
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tls/routes_mgr_test.rs"]
mod tests;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::tls::gateway_tls_routes::TlsRouteTable;
use crate::types::resources::TLSRoute;
use crate::types::ListenerKey;
use crate::types::ResourceMeta;

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

    /// Rebuild the route table from routes belonging to this listener.
    /// Called by `GlobalTlsRouteManagers` during rebuild.
    pub fn rebuild(&self, routes: &HashMap<String, Arc<TLSRoute>>) {
        let new_table = TlsRouteTable::from_routes(routes);

        tracing::debug!(
            component = "tls_route_manager",
            routes = routes.len(),
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
    /// Canonical route store: resource_key -> initialized Arc<TLSRoute>
    route_cache: DashMap<String, Arc<TLSRoute>>,

    /// listener -> per-listener manager (stable Arc held by listeners)
    by_listener: DashMap<ListenerKey, Arc<TlsRouteManager>>,
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
        }
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

    /// Insert an initialized route into the cache.
    pub fn insert_route(&self, route: Arc<TLSRoute>) {
        let key = route.key_name();
        self.route_cache.insert(key, route);
    }

    /// Get a route from the cache by key.
    pub fn get_route(&self, key: &str) -> Option<Arc<TLSRoute>> {
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
    /// 1. Bucket each route by its target listeners (from resolved_listeners)
    /// 2. For each listener that has routes: rebuild the manager
    /// 3. For listeners that no longer have routes: rebuild with empty set
    pub fn rebuild_all_listener_managers(&self) {
        // Pre-create listener managers for all known listeners from PortGatewayInfoStore
        let pgis = crate::core::gateway::runtime::store::get_port_gateway_info_store();
        for key in pgis.all_listener_keys() {
            self.get_or_create_listener_manager(&key);
        }

        let listener_buckets = self.bucket_routes_by_listener();

        let mut rebuilt_listeners = 0u32;

        for (key, routes) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            manager.rebuild(routes);
            rebuilt_listeners += 1;
        }

        // Remove stale listener entries that no longer have any routes and are not
        // registered in PortGatewayInfoStore
        let active_listeners: HashSet<ListenerKey> = pgis.all_listener_keys().into_iter().collect();
        let stale_listeners: Vec<ListenerKey> = self
            .by_listener
            .iter()
            .filter(|e| !listener_buckets.contains_key(e.key()) && !active_listeners.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        for key in &stale_listeners {
            self.by_listener.remove(key);
        }

        let total_routes: usize = listener_buckets.values().map(|r| r.len()).sum();
        tracing::info!(
            component = "global_tls_route_managers",
            listeners = listener_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_listeners,
            removed_stale_listeners = stale_listeners.len(),
            "Rebuilt all per-listener TLS route managers"
        );
    }

    /// Rebuild only the per-listener managers for the given set of affected listeners.
    ///
    /// Single-pass: iterates route_cache once, bucketing routes into only the
    /// affected listeners. Listeners in `affected_listeners` that end up with no routes
    /// are rebuilt with an empty set (clearing stale entries).
    pub fn rebuild_affected_listener_managers(&self, affected_listeners: &HashSet<ListenerKey>) {
        if affected_listeners.is_empty() {
            return;
        }

        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<TLSRoute>>> = HashMap::new();
        for key in affected_listeners {
            listener_buckets.insert(key.clone(), HashMap::new());
        }

        for entry in self.route_cache.iter() {
            let route_listeners = resolved_listeners_for_route(entry.value());
            for key in route_listeners {
                if let Some(bucket) = listener_buckets.get_mut(key) {
                    bucket.insert(entry.key().clone(), entry.value().clone());
                }
            }
        }

        let pgis = crate::core::gateway::runtime::store::get_port_gateway_info_store();
        let active_listeners: HashSet<ListenerKey> = pgis.all_listener_keys().into_iter().collect();
        let mut removed_stale = 0usize;
        for (key, routes) in &listener_buckets {
            let manager = self.get_or_create_listener_manager(key);
            manager.rebuild(routes);
            if routes.is_empty() && !active_listeners.contains(key) {
                self.by_listener.remove(key);
                removed_stale += 1;
            }
        }

        tracing::info!(
            component = "global_tls_route_managers",
            affected = affected_listeners.len(),
            removed_stale_listeners = removed_stale,
            "Rebuilt affected per-listener TLS route managers"
        );
    }

    /// Bucket all cached routes by their resolved listeners.
    fn bucket_routes_by_listener(&self) -> HashMap<ListenerKey, HashMap<String, Arc<TLSRoute>>> {
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<TLSRoute>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let listeners = resolved_listeners_for_route(&route);

            if listeners.is_empty() {
                tracing::warn!(
                    route = %route_key,
                    "TLSRoute has no resolved_listeners, skipping listener assignment"
                );
                continue;
            }

            for key in listeners {
                listener_buckets
                    .entry(key.clone())
                    .or_default()
                    .insert(route_key.clone(), route.clone());
            }
        }

        listener_buckets
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

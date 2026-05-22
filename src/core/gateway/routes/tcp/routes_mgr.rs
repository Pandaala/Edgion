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
        }
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
    /// 1. Bucket each route by its target listeners (from resolved_listeners)
    /// 2. For each listener that has routes: rebuild the manager
    /// 3. For listeners that no longer have routes: rebuild with empty set
    pub fn rebuild_all_listener_managers(&self) {
        // Pre-create listener managers for all known listeners from ListenerGatewayInfoStore
        let pgis = crate::core::gateway::runtime::store::get_listener_gateway_info_store();
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
        // registered in ListenerGatewayInfoStore
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
            component = "global_tcp_route_managers",
            listeners = listener_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_listeners,
            removed_stale_listeners = stale_listeners.len(),
            "Rebuilt all per-listener TCP route managers"
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

        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<TCPRoute>>> = HashMap::new();
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

        let pgis = crate::core::gateway::runtime::store::get_listener_gateway_info_store();
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

        // Sweep orphan entries left over from earlier rebuild_all calls — see the
        // matching comment in HTTP routes_mgr::rebuild_affected_listener_managers.
        let route_referenced: HashSet<ListenerKey> = self
            .route_cache
            .iter()
            .flat_map(|e| resolved_listeners_for_route(e.value()).to_vec())
            .collect();
        let orphans: Vec<ListenerKey> = self
            .by_listener
            .iter()
            .filter(|e| !route_referenced.contains(e.key()) && !active_listeners.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        for key in &orphans {
            self.by_listener.remove(key);
        }
        removed_stale += orphans.len();

        tracing::info!(
            component = "global_tcp_route_managers",
            affected = affected_listeners.len(),
            removed_stale_listeners = removed_stale,
            "Rebuilt affected per-listener TCP route managers"
        );
    }

    /// Bucket all cached routes by their resolved listeners.
    fn bucket_routes_by_listener(&self) -> HashMap<ListenerKey, HashMap<String, Arc<TCPRoute>>> {
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<TCPRoute>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let listeners = resolved_listeners_for_route(&route);

            if listeners.is_empty() {
                tracing::warn!(
                    route = %route_key,
                    "TCPRoute has no resolved_listeners, skipping listener assignment"
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

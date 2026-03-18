use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::tcp::tcp_route_table::TcpRouteTable;
use crate::types::resources::TCPRoute;
use crate::types::ResourceMeta;

/// Per-port TCP route manager.
///
/// Each instance owns an atomically-swappable `TcpRouteTable` snapshot.
/// `EdgionTcpProxy` holds an `Arc<TcpPortRouteManager>` and calls
/// `load_route_table()` per-connection for lock-free lookups.
///
/// Route data is owned by `GlobalTcpRouteManagers`; this struct only
/// holds the compiled route snapshot for one port.
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

    /// Rebuild the route table from routes belonging to this port.
    /// Called by `GlobalTcpRouteManagers` during rebuild.
    pub fn rebuild(&self, routes: &HashMap<String, Arc<TCPRoute>>) {
        let new_table = TcpRouteTable::from_routes(routes);

        tracing::debug!(
            component = "tcp_port_route_manager",
            routes = routes.len(),
            "Rebuilt per-port TCP route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `port -> Arc<TcpPortRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> Arc<TCPRoute>`) and
/// implements `ConfHandler<TCPRoute>`. On every change it rebuilds all
/// per-port managers by resolving each route's target ports from its
/// `parentRefs`.
pub struct GlobalTcpRouteManagers {
    /// Canonical route store: resource_key -> initialized Arc<TCPRoute>
    route_cache: DashMap<String, Arc<TCPRoute>>,

    /// port -> per-port manager (stable Arc held by listeners)
    by_port: DashMap<u16, Arc<TcpPortRouteManager>>,
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
            by_port: DashMap::new(),
        }
    }

    /// Get or create a per-port `TcpPortRouteManager`.
    ///
    /// Called by `listener_builder` at startup. The returned `Arc` is stable;
    /// route updates only swap the inner `ArcSwap<TcpRouteTable>`.
    pub fn get_or_create_port_manager(&self, port: u16) -> Arc<TcpPortRouteManager> {
        self.by_port
            .entry(port)
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

    /// Rebuild all per-port managers from the current route cache.
    ///
    /// 1. Bucket each route by its target ports (from parentRef)
    /// 2. For each port that has routes: rebuild the manager
    /// 3. For ports that no longer have routes: rebuild with empty set
    pub fn rebuild_all_port_managers(&self) {
        let port_buckets = self.bucket_routes_by_port();

        let mut rebuilt_ports = 0u32;

        for (port, routes) in &port_buckets {
            let manager = self.get_or_create_port_manager(*port);
            manager.rebuild(routes);
            rebuilt_ports += 1;
        }

        // Remove stale port entries that no longer have any routes
        let stale_ports: Vec<u16> = self
            .by_port
            .iter()
            .filter(|e| !port_buckets.contains_key(e.key()))
            .map(|e| *e.key())
            .collect();
        for port in &stale_ports {
            self.by_port.remove(port);
        }

        let total_routes: usize = port_buckets.values().map(|r| r.len()).sum();
        tracing::info!(
            component = "global_tcp_route_managers",
            ports = port_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_ports,
            removed_stale_ports = stale_ports.len(),
            "Rebuilt all per-port TCP route managers"
        );
    }

    /// Rebuild only the per-port managers for the given set of affected ports.
    ///
    /// Single-pass: iterates route_cache once, bucketing routes into only the
    /// affected ports. Ports in `affected_ports` that end up with no routes
    /// are rebuilt with an empty set (clearing stale entries).
    pub fn rebuild_affected_port_managers(&self, affected_ports: &HashSet<u16>) {
        if affected_ports.is_empty() {
            return;
        }

        let mut port_buckets: HashMap<u16, HashMap<String, Arc<TCPRoute>>> = HashMap::new();
        for &port in affected_ports {
            port_buckets.insert(port, HashMap::new());
        }

        for entry in self.route_cache.iter() {
            let route_ports = resolved_ports_for_route(entry.value());
            for &port in route_ports {
                if let Some(bucket) = port_buckets.get_mut(&port) {
                    bucket.insert(entry.key().clone(), entry.value().clone());
                }
            }
        }

        let mut removed_stale = 0usize;
        for (port, routes) in &port_buckets {
            let manager = self.get_or_create_port_manager(*port);
            manager.rebuild(routes);
            if routes.is_empty() {
                self.by_port.remove(port);
                removed_stale += 1;
            }
        }

        tracing::info!(
            component = "global_tcp_route_managers",
            affected = affected_ports.len(),
            removed_stale_ports = removed_stale,
            "Rebuilt affected per-port TCP route managers"
        );
    }

    /// Bucket all cached routes by their resolved ports.
    fn bucket_routes_by_port(&self) -> HashMap<u16, HashMap<String, Arc<TCPRoute>>> {
        let mut port_buckets: HashMap<u16, HashMap<String, Arc<TCPRoute>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let ports = resolved_ports_for_route(&route);

            if ports.is_empty() {
                tracing::warn!(
                    route = %route_key,
                    "TCPRoute has no resolved_ports, skipping port assignment"
                );
                continue;
            }

            for &port in ports {
                port_buckets
                    .entry(port)
                    .or_default()
                    .insert(route_key.clone(), route.clone());
            }
        }

        port_buckets
    }
}

/// Get the resolved listener ports for a TCPRoute.
///
/// Uses `spec.resolved_ports` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.port.
pub(crate) fn resolved_ports_for_route(route: &TCPRoute) -> &[u16] {
    route.spec.resolved_ports.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TcpRouteManagerStats {
    pub route_cache: usize,
    pub port_count: usize,
}

impl GlobalTcpRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> TcpRouteManagerStats {
        TcpRouteManagerStats {
            route_cache: self.route_cache.len(),
            port_count: self.by_port.len(),
        }
    }
}

static GLOBAL_TCP_ROUTE_MANAGERS: OnceLock<GlobalTcpRouteManagers> = OnceLock::new();

pub fn get_global_tcp_route_managers() -> &'static GlobalTcpRouteManagers {
    GLOBAL_TCP_ROUTE_MANAGERS.get_or_init(GlobalTcpRouteManagers::new)
}

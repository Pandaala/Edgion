use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::tls::gateway_tls_routes::TlsRouteTable;
use crate::types::resources::TLSRoute;
use crate::types::ResourceMeta;

/// Per-port TLS route manager.
///
/// Each instance owns an atomically-swappable `TlsRouteTable` snapshot.
/// `EdgionTlsTcpProxy` holds an `Arc<TlsRouteManager>` and calls
/// `load_route_table()` per-connection for lock-free lookups.
///
/// Route data is owned by `GlobalTlsRouteManagers`; this struct only
/// holds the compiled hostname-index snapshot for one port.
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

    /// Rebuild the route table from routes belonging to this port.
    /// Called by `GlobalTlsRouteManagers` during rebuild.
    pub fn rebuild(&self, routes: &HashMap<String, Arc<TLSRoute>>) {
        let new_table = TlsRouteTable::from_routes(routes);

        tracing::debug!(
            component = "tls_route_manager",
            routes = routes.len(),
            catch_all = new_table.has_catch_all(),
            "Rebuilt per-port TLS route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `port -> Arc<TlsRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> Arc<TLSRoute>`) and
/// implements `ConfHandler<TLSRoute>`.  On every change it rebuilds all
/// per-port managers by resolving each route's target ports from its
/// `parentRefs`.
pub struct GlobalTlsRouteManagers {
    /// Canonical route store: resource_key -> initialized Arc<TLSRoute>
    route_cache: DashMap<String, Arc<TLSRoute>>,

    /// port -> per-port manager (stable Arc held by listeners)
    by_port: DashMap<u16, Arc<TlsRouteManager>>,
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
            by_port: DashMap::new(),
        }
    }

    /// Get or create a per-port `TlsRouteManager`.
    ///
    /// Called by `listener_builder` at startup. The returned `Arc` is stable;
    /// route updates only swap the inner `ArcSwap<TlsRouteTable>`.
    pub fn get_or_create_port_manager(&self, port: u16) -> Arc<TlsRouteManager> {
        self.by_port
            .entry(port)
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

        for entry in self.by_port.iter() {
            let port = *entry.key();
            if !port_buckets.contains_key(&port) {
                entry.value().rebuild(&HashMap::new());
                rebuilt_ports += 1;
            }
        }

        let total_routes: usize = port_buckets.values().map(|r| r.len()).sum();
        tracing::info!(
            component = "global_tls_route_managers",
            ports = port_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_ports,
            "Rebuilt all per-port TLS route managers"
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

        let mut port_buckets: HashMap<u16, HashMap<String, Arc<TLSRoute>>> = HashMap::new();
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

        for (port, routes) in &port_buckets {
            let manager = self.get_or_create_port_manager(*port);
            manager.rebuild(routes);
        }

        tracing::info!(
            component = "global_tls_route_managers",
            affected = affected_ports.len(),
            "Rebuilt affected per-port TLS route managers"
        );
    }

    /// Bucket all cached routes by their resolved ports.
    fn bucket_routes_by_port(&self) -> HashMap<u16, HashMap<String, Arc<TLSRoute>>> {
        let mut port_buckets: HashMap<u16, HashMap<String, Arc<TLSRoute>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let ports = resolved_ports_for_route(&route);

            if ports.is_empty() {
                tracing::warn!(
                    route = %route_key,
                    "TLSRoute has no resolved_ports, skipping port assignment"
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

/// Get the resolved listener ports for a TLSRoute.
///
/// Uses `spec.resolved_ports` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.port.
pub(crate) fn resolved_ports_for_route(route: &TLSRoute) -> &[u16] {
    route.spec.resolved_ports.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TlsRouteManagerStats {
    pub route_cache: usize,
    pub port_count: usize,
}

impl GlobalTlsRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> TlsRouteManagerStats {
        TlsRouteManagerStats {
            route_cache: self.route_cache.len(),
            port_count: self.by_port.len(),
        }
    }
}

static GLOBAL_TLS_ROUTE_MANAGERS: OnceLock<GlobalTlsRouteManagers> = OnceLock::new();

pub fn get_global_tls_route_managers() -> &'static GlobalTlsRouteManagers {
    GLOBAL_TLS_ROUTE_MANAGERS.get_or_init(GlobalTlsRouteManagers::new)
}

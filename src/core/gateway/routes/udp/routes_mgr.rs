use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::udp::udp_route_table::UdpRouteTable;
use crate::types::resources::UDPRoute;
use crate::types::ResourceMeta;

/// Per-port UDP route manager.
///
/// Each instance owns an atomically-swappable `UdpRouteTable` snapshot.
/// `EdgionUdpProxy` holds an `Arc<UdpPortRouteManager>` and calls
/// `load_route_table()` per-packet for lock-free lookups.
///
/// Route data is owned by `GlobalUdpRouteManagers`; this struct only
/// holds the compiled route snapshot for one port.
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

    /// Load the current route table snapshot (hot path, per-packet).
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<UdpRouteTable>> {
        self.route_table.load()
    }

    /// Rebuild the route table from routes belonging to this port.
    /// Called by `GlobalUdpRouteManagers` during rebuild.
    pub fn rebuild(&self, routes: &HashMap<String, Arc<UDPRoute>>) {
        let new_table = UdpRouteTable::from_routes(routes);

        tracing::debug!(
            component = "udp_port_route_manager",
            routes = routes.len(),
            "Rebuilt per-port UDP route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `port -> Arc<UdpPortRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> Arc<UDPRoute>`) and
/// implements `ConfHandler<UDPRoute>`. On every change it rebuilds all
/// per-port managers by resolving each route's target ports from its
/// `parentRefs`.
pub struct GlobalUdpRouteManagers {
    /// Canonical route store: resource_key -> initialized Arc<UDPRoute>
    route_cache: DashMap<String, Arc<UDPRoute>>,

    /// port -> per-port manager (stable Arc held by listeners)
    by_port: DashMap<u16, Arc<UdpPortRouteManager>>,
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
            by_port: DashMap::new(),
        }
    }

    /// Get or create a per-port `UdpPortRouteManager`.
    ///
    /// Called by `listener_builder` at startup. The returned `Arc` is stable;
    /// route updates only swap the inner `ArcSwap<UdpRouteTable>`.
    pub fn get_or_create_port_manager(&self, port: u16) -> Arc<UdpPortRouteManager> {
        self.by_port
            .entry(port)
            .or_insert_with(|| Arc::new(UdpPortRouteManager::new()))
            .value()
            .clone()
    }

    /// Insert an initialized route into the cache.
    pub fn insert_route(&self, route: Arc<UDPRoute>) {
        let key = route.key_name();
        self.route_cache.insert(key, route);
    }

    /// Get a route from the cache by key.
    pub fn get_route(&self, key: &str) -> Option<Arc<UDPRoute>> {
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
            component = "global_udp_route_managers",
            ports = port_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_ports,
            removed_stale_ports = stale_ports.len(),
            "Rebuilt all per-port UDP route managers"
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

        let mut port_buckets: HashMap<u16, HashMap<String, Arc<UDPRoute>>> = HashMap::new();
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
            component = "global_udp_route_managers",
            affected = affected_ports.len(),
            removed_stale_ports = removed_stale,
            "Rebuilt affected per-port UDP route managers"
        );
    }

    /// Bucket all cached routes by their resolved ports.
    fn bucket_routes_by_port(&self) -> HashMap<u16, HashMap<String, Arc<UDPRoute>>> {
        let mut port_buckets: HashMap<u16, HashMap<String, Arc<UDPRoute>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let ports = resolved_ports_for_route(&route);

            if ports.is_empty() {
                tracing::warn!(
                    route = %route_key,
                    "UDPRoute has no resolved_ports, skipping port assignment"
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

/// Get the resolved listener ports for a UDPRoute.
///
/// Uses `spec.resolved_ports` which is pre-computed by the controller
/// from parentRef.port / parentRef.sectionName → Gateway listener.port.
pub(crate) fn resolved_ports_for_route(route: &UDPRoute) -> &[u16] {
    route.spec.resolved_ports.as_deref().unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UdpRouteManagerStats {
    pub route_cache: usize,
    pub port_count: usize,
}

impl GlobalUdpRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> UdpRouteManagerStats {
        UdpRouteManagerStats {
            route_cache: self.route_cache.len(),
            port_count: self.by_port.len(),
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
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::udp_route::*;

    fn create_test_udp_route(namespace: &str, name: &str, port: u16) -> UDPRoute {
        UDPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: UDPRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: Some("udp".to_string()),
                    port: Some(port as i32),
                }]),
                rules: Some(vec![]),
                resolved_ports: Some(vec![port]),
            },
            status: None,
        }
    }

    #[test]
    fn test_get_or_create_port_manager() {
        let managers = GlobalUdpRouteManagers::new();
        let mgr1 = managers.get_or_create_port_manager(9000);
        let mgr2 = managers.get_or_create_port_manager(9000);
        assert!(Arc::ptr_eq(&mgr1, &mgr2));

        let mgr3 = managers.get_or_create_port_manager(9001);
        assert!(!Arc::ptr_eq(&mgr1, &mgr3));
    }

    #[test]
    fn test_insert_and_get_route() {
        let managers = GlobalUdpRouteManagers::new();
        let route = Arc::new(create_test_udp_route("default", "route1", 9000));
        let key = route.key_name();

        managers.insert_route(route.clone());
        let found = managers.get_route(&key);
        assert!(found.is_some());

        managers.remove_route(&key);
        let found = managers.get_route(&key);
        assert!(found.is_none());
    }

    #[test]
    fn test_rebuild_all_port_managers() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);

        let route = Arc::new(create_test_udp_route("default", "route1", 9000));
        managers.insert_route(route);

        managers.rebuild_all_port_managers();

        let mgr = managers.get_or_create_port_manager(9000);
        let table = mgr.load_route_table();
        assert!(table.match_route().is_some());
    }

    #[test]
    fn test_rebuild_affected_port_managers() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);
        let _ = managers.get_or_create_port_manager(9001);

        let route = Arc::new(create_test_udp_route("default", "route1", 9000));
        managers.insert_route(route);

        let mut affected = HashSet::new();
        affected.insert(9000);
        managers.rebuild_affected_port_managers(&affected);

        let mgr9000 = managers.get_or_create_port_manager(9000);
        assert!(mgr9000.load_route_table().match_route().is_some());

        let mgr9001 = managers.get_or_create_port_manager(9001);
        assert!(mgr9001.load_route_table().match_route().is_none());
    }

    #[test]
    fn test_stats() {
        let managers = GlobalUdpRouteManagers::new();
        let route = Arc::new(create_test_udp_route("default", "route1", 9000));
        managers.insert_route(route);
        let _ = managers.get_or_create_port_manager(9000);

        let stats = managers.stats();
        assert_eq!(stats.route_cache, 1);
        assert_eq!(stats.port_count, 1);
    }

    #[test]
    fn test_clear_route_cache() {
        let managers = GlobalUdpRouteManagers::new();
        let route = Arc::new(create_test_udp_route("default", "route1", 9000));
        managers.insert_route(route);

        assert_eq!(managers.stats().route_cache, 1);
        managers.clear_route_cache();
        assert_eq!(managers.stats().route_cache, 0);
    }
}

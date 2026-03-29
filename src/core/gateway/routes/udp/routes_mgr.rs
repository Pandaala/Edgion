use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::udp::udp_route_table::UdpRouteTable;
use crate::types::resources::UDPRoute;
use crate::types::ListenerKey;
use crate::types::ResourceMeta;

/// Per-listener UDP route manager.
///
/// Each instance owns an atomically-swappable `UdpRouteTable` snapshot.
/// `EdgionUdpProxy` holds an `Arc<UdpPortRouteManager>` and calls
/// `load_route_table()` per-packet for lock-free lookups.
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

    /// Load the current route table snapshot (hot path, per-packet).
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<UdpRouteTable>> {
        self.route_table.load()
    }

    /// Rebuild the route table from routes belonging to this listener.
    /// Called by `GlobalUdpRouteManagers` during rebuild.
    pub fn rebuild(&self, routes: &HashMap<String, Arc<UDPRoute>>) {
        let new_table = UdpRouteTable::from_routes(routes);

        tracing::debug!(
            component = "udp_port_route_manager",
            routes = routes.len(),
            "Rebuilt per-listener UDP route table"
        );

        self.route_table.store(Arc::new(new_table));
    }
}

/// Global wrapper managing `ListenerKey -> Arc<UdpPortRouteManager>`.
///
/// Owns the canonical route cache (`resource_key -> Arc<UDPRoute>`) and
/// implements `ConfHandler<UDPRoute>`. On every change it rebuilds all
/// per-listener managers by resolving each route's target listeners from its
/// `spec.resolved_listeners`.
pub struct GlobalUdpRouteManagers {
    /// Canonical route store: resource_key -> initialized Arc<UDPRoute>
    route_cache: DashMap<String, Arc<UDPRoute>>,

    /// listener -> per-listener manager (stable Arc held by listeners)
    by_listener: DashMap<ListenerKey, Arc<UdpPortRouteManager>>,
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
        }
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
            component = "global_udp_route_managers",
            listeners = listener_buckets.len(),
            total_route_entries = total_routes,
            rebuilt_listeners,
            removed_stale_listeners = stale_listeners.len(),
            "Rebuilt all per-listener UDP route managers"
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

        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<UDPRoute>>> = HashMap::new();
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

        tracing::info!(
            component = "global_udp_route_managers",
            affected = affected_listeners.len(),
            removed_stale_listeners = removed_stale,
            "Rebuilt affected per-listener UDP route managers"
        );
    }

    /// Bucket all cached routes by their resolved listeners.
    fn bucket_routes_by_listener(&self) -> HashMap<ListenerKey, HashMap<String, Arc<UDPRoute>>> {
        let mut listener_buckets: HashMap<ListenerKey, HashMap<String, Arc<UDPRoute>>> = HashMap::new();

        for entry in self.route_cache.iter() {
            let route_key = entry.key().clone();
            let route = entry.value().clone();
            let listeners = resolved_listeners_for_route(&route);

            if listeners.is_empty() {
                tracing::warn!(
                    route = %route_key,
                    "UDPRoute has no resolved_listeners, skipping listener assignment"
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
}

impl GlobalUdpRouteManagers {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> UdpRouteManagerStats {
        UdpRouteManagerStats {
            route_cache: self.route_cache.len(),
            listener_count: self.by_listener.len(),
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
    use crate::types::resources::udp_route::*;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_udp_route(namespace: &str, name: &str, listener_key: ListenerKey) -> UDPRoute {
        use crate::types::resources::common::ParentReference;
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
                    port: None,
                }]),
                rules: Some(vec![]),
                resolved_listeners: Some(vec![listener_key]),
            },
            status: None,
        }
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
        let route = Arc::new(create_test_udp_route("default", "route1", key));
        let route_key = route.key_name();

        managers.insert_route(route.clone());
        let found = managers.get_route(&route_key);
        assert!(found.is_some());

        managers.remove_route(&route_key);
        let found = managers.get_route(&route_key);
        assert!(found.is_none());
    }

    #[test]
    fn test_stats() {
        let managers = GlobalUdpRouteManagers::new();
        let key = make_listener_key(9000, "UDP");
        let route = Arc::new(create_test_udp_route("default", "route1", key.clone()));
        managers.insert_route(route);
        let _ = managers.get_or_create_listener_manager(&key);

        let stats = managers.stats();
        assert_eq!(stats.route_cache, 1);
        assert_eq!(stats.listener_count, 1);
    }

    #[test]
    fn test_clear_route_cache() {
        let managers = GlobalUdpRouteManagers::new();
        let key = make_listener_key(9000, "UDP");
        let route = Arc::new(create_test_udp_route("default", "route1", key));
        managers.insert_route(route);

        assert_eq!(managers.stats().route_cache, 1);
        managers.clear_route_cache();
        assert_eq!(managers.stats().route_cache, 0);
    }
}

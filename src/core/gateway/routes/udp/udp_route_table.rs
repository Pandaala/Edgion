use crate::types::resources::UDPRoute;
use crate::types::ResourceMeta;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-port UDP route table — immutable snapshot shared via ArcSwap.
///
/// UDP listeners have simpler semantics than TLS: there is no hostname-based
/// matching (no SNI equivalent). Per Gateway API, each UDP listener binds at
/// most one UDPRoute, so the table stores a flat list with first-match
/// semantics.
///
/// A new snapshot is built and atomically swapped on every route change,
/// ensuring all readers always see consistent data without locking.
pub struct UdpRouteTable {
    routes: Vec<Arc<UDPRoute>>,
}

impl UdpRouteTable {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Build a UdpRouteTable from a flat set of routes belonging to one port.
    ///
    /// Sort by resource key (`namespace/name`) so `match_route`'s first-match
    /// pick is deterministic across `HashMap` iteration order, process restarts,
    /// and pod replicas. Per Gateway API a UDP listener binds at most one route;
    /// the sort only matters as a stable fallback when that invariant is violated
    /// (operator misconfig or a controller event race), mirroring the
    /// `AlphabeticalRoute` tiebreak.
    pub fn from_routes(routes: &HashMap<String, Arc<UDPRoute>>) -> Self {
        let mut routes: Vec<Arc<UDPRoute>> = routes.values().cloned().collect();
        routes.sort_by_key(|a| a.key_name());
        Self { routes }
    }

    /// Match a UDPRoute (first-match semantics).
    ///
    /// UDP has no hostname dimension, so we simply return the first available
    /// route for this port.
    pub fn match_route(&self) -> Option<Arc<UDPRoute>> {
        self.routes.first().cloned()
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl Default for UdpRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::udp_route::*;
    use crate::types::ResourceMeta;

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
                resolved_listeners: None,
            },
            status: None,
        }
    }

    #[test]
    fn test_match_route_returns_first() {
        let route = Arc::new(create_test_udp_route("default", "route1", 9000));
        let mut routes = HashMap::new();
        routes.insert(route.key_name(), route.clone());

        let table = UdpRouteTable::from_routes(&routes);
        let matched = table.match_route();
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().key_name(), route.key_name());
    }

    #[test]
    fn test_empty_table() {
        let table = UdpRouteTable::new();
        assert!(table.match_route().is_none());
        assert!(table.is_empty());
        assert_eq!(table.route_count(), 0);
    }

    #[test]
    fn test_match_route_is_deterministic() {
        // Two routes on the same listener (invariant violation). Insert in the
        // order "b/route" then "a/route"; match_route must return "a/route"
        // regardless of HashMap iteration order, proving the build-time sort.
        let route_b = Arc::new(create_test_udp_route("b", "route", 9000));
        let route_a = Arc::new(create_test_udp_route("a", "route", 9000));

        let mut routes = HashMap::new();
        routes.insert(route_b.key_name(), route_b);
        routes.insert(route_a.key_name(), route_a);

        let table = UdpRouteTable::from_routes(&routes);
        assert_eq!(table.route_count(), 2);
        assert_eq!(table.match_route().unwrap().key_name(), "a/route");
    }

    #[test]
    fn test_from_routes() {
        let route1 = Arc::new(create_test_udp_route("default", "route1", 9000));
        let route2 = Arc::new(create_test_udp_route("default", "route2", 9000));

        let mut routes = HashMap::new();
        routes.insert(route1.key_name(), route1);
        routes.insert(route2.key_name(), route2);

        let table = UdpRouteTable::from_routes(&routes);
        assert_eq!(table.route_count(), 2);
        assert!(table.match_route().is_some());
    }
}

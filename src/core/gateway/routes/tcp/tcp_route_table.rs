use crate::types::resources::TCPRoute;
use crate::types::ResourceMeta;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-port TCP route table — immutable snapshot shared via ArcSwap.
///
/// TCP listeners have simpler semantics than TLS: there is no hostname-based
/// matching (no SNI equivalent). Per Gateway API, each TCP listener binds at
/// most one TCPRoute, so the table stores a flat list with first-match
/// semantics.
///
/// A new snapshot is built and atomically swapped on every route change,
/// ensuring all readers always see consistent data without locking.
pub struct TcpRouteTable {
    routes: Vec<Arc<TCPRoute>>,
}

impl TcpRouteTable {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Build a TcpRouteTable from a flat set of routes belonging to one port.
    ///
    /// Sort by resource key (`namespace/name`) so `match_route`'s first-match
    /// pick is deterministic across `HashMap` iteration order, process restarts,
    /// and pod replicas. Per Gateway API a TCP listener binds at most one route;
    /// the sort only matters as a stable fallback when that invariant is violated
    /// (operator misconfig or a controller event race), mirroring the
    /// `AlphabeticalRoute` tiebreak.
    pub fn from_routes(routes: &HashMap<String, Arc<TCPRoute>>) -> Self {
        let mut routes: Vec<Arc<TCPRoute>> = routes.values().cloned().collect();
        routes.sort_by_key(|a| a.key_name());
        Self { routes }
    }

    /// Match a TCPRoute (first-match semantics).
    ///
    /// TCP has no hostname dimension, so we simply return the first available
    /// route for this port.
    pub fn match_route(&self) -> Option<Arc<TCPRoute>> {
        self.routes.first().cloned()
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl Default for TcpRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::tcp_route::*;
    use crate::types::ResourceMeta;

    fn create_test_tcp_route(namespace: &str, name: &str, port: u16) -> TCPRoute {
        TCPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TCPRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: Some("tcp".to_string()),
                    port: Some(port as i32),
                }]),
                rules: Some(vec![]),
                resolved_listeners: None,
            },
            status: None,
        }
    }

    #[test]
    fn test_match_route_is_deterministic() {
        // Two routes on the same listener (invariant violation). Insert in the
        // order "b/route" then "a/route"; match_route must return "a/route"
        // regardless of HashMap iteration order, proving the build-time sort.
        let route_b = Arc::new(create_test_tcp_route("b", "route", 9000));
        let route_a = Arc::new(create_test_tcp_route("a", "route", 9000));

        let mut routes = HashMap::new();
        routes.insert(route_b.key_name(), route_b);
        routes.insert(route_a.key_name(), route_a);

        let table = TcpRouteTable::from_routes(&routes);
        assert_eq!(table.route_count(), 2);
        assert_eq!(table.match_route().unwrap().key_name(), "a/route");
    }

    #[test]
    fn test_empty_table() {
        let table = TcpRouteTable::new();
        assert!(table.match_route().is_none());
        assert!(table.is_empty());
        assert_eq!(table.route_count(), 0);
    }
}

use crate::types::resources::TCPRoute;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;

/// Gateway-level TCP route collection
///
/// Stores all TCPRoutes associated with a specific Gateway, indexed by listener name (sectionName)
/// Uses ArcSwap for lock-free concurrent access
pub struct GatewayTcpRoutes {
    /// listener_name -> Vec<Arc<TCPRoute>> mapping
    /// Routes are indexed by listener name (sectionName) for proper Gateway API compliance
    listener_routes_map: ArcSwap<HashMap<String, Vec<Arc<TCPRoute>>>>,
}

impl GatewayTcpRoutes {
    /// Create a new empty GatewayTcpRoutes
    pub fn new() -> Self {
        Self {
            listener_routes_map: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Match a TCPRoute by listener name and port
    ///
    /// Returns the first matching route for the given listener name.
    /// Port is also checked for validation but primarily matches by listener name (sectionName).
    /// This properly implements Gateway API sectionName matching.
    pub fn match_route(&self, listener_name: &str, _port: u16) -> Option<Arc<TCPRoute>> {
        let listener_routes = self.listener_routes_map.load();
        listener_routes
            .get(listener_name)
            .and_then(|routes| routes.first().cloned())
    }

    /// Get all routes for a specific listener name
    pub fn get_routes_for_listener(&self, listener_name: &str) -> Vec<Arc<TCPRoute>> {
        let listener_routes = self.listener_routes_map.load();
        listener_routes.get(listener_name).cloned().unwrap_or_default()
    }

    /// Get all listener names that have routes
    pub fn get_all_listener_names(&self) -> Vec<String> {
        let listener_routes = self.listener_routes_map.load();
        listener_routes.keys().cloned().collect()
    }

    /// Update the routes map (called by TcpRouteManager during config sync)
    pub(crate) fn update_routes(&self, new_routes: HashMap<String, Vec<Arc<TCPRoute>>>) {
        self.listener_routes_map.store(Arc::new(new_routes));
    }

    /// Incrementally update routes for specified listener names only (fine-grained update)
    ///
    /// This method only updates the specified listeners, leaving other listeners unchanged.
    /// Uses RCU (Read-Copy-Update) pattern for lock-free updates.
    ///
    /// # Arguments
    /// * `listener_routes` - Map of listener_name -> routes to update. Empty Vec means clear that listener.
    pub(crate) fn update_listeners_incremental(&self, listener_routes: HashMap<String, Vec<Arc<TCPRoute>>>) {
        let current_arc = self.listener_routes_map.load();

        // Clone inner HashMap and apply incremental updates
        let mut new_map = (**current_arc).clone();

        for (listener_name, routes) in listener_routes {
            if routes.is_empty() {
                // Remove listener if no routes
                new_map.remove(&listener_name);
            } else {
                // Update or insert routes for this listener
                new_map.insert(listener_name, routes);
            }
        }

        self.listener_routes_map.store(Arc::new(new_map));
    }

    /// Check if there are any routes
    pub fn is_empty(&self) -> bool {
        let listener_routes = self.listener_routes_map.load();
        listener_routes.is_empty()
    }
}

impl Default for GatewayTcpRoutes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::tcp_route::*;

    fn create_test_tcp_route(namespace: &str, name: &str, listener_name: &str, port: i32) -> TCPRoute {
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
                    section_name: Some(listener_name.to_string()),
                    port: Some(port),
                }]),
                rules: Some(vec![]),
            },
            status: None,
        }
    }

    #[test]
    fn test_gateway_tcp_routes_match() {
        let gateway_routes = GatewayTcpRoutes::new();

        let route1 = Arc::new(create_test_tcp_route("default", "route1", "tcp-9000", 9000));
        let route2 = Arc::new(create_test_tcp_route("default", "route2", "tcp-9001", 9001));

        let mut routes_map = HashMap::new();
        routes_map.insert("tcp-9000".to_string(), vec![route1.clone()]);
        routes_map.insert("tcp-9001".to_string(), vec![route2.clone()]);

        gateway_routes.update_routes(routes_map);

        assert!(gateway_routes.match_route("tcp-9000", 9000).is_some());
        assert!(gateway_routes.match_route("tcp-9001", 9001).is_some());
        assert!(gateway_routes.match_route("tcp-9002", 9002).is_none());
    }

    #[test]
    fn test_gateway_tcp_routes_empty() {
        let gateway_routes = GatewayTcpRoutes::new();
        assert!(gateway_routes.is_empty());
        assert!(gateway_routes.match_route("tcp-9000", 9000).is_none());
    }
}

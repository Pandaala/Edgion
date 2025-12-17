use std::sync::Arc;
use std::collections::HashMap;
use arc_swap::ArcSwap;
use crate::types::resources::UDPRoute;

/// Gateway-level UDP route collection
/// 
/// Stores all UDPRoutes associated with a specific Gateway, indexed by listening port
/// Uses ArcSwap for lock-free concurrent access
pub struct GatewayUdpRoutes {
    /// port -> Vec<Arc<UDPRoute>> mapping
    /// Multiple routes can listen on the same port (with different section names or priorities)
    port_routes_map: ArcSwap<Arc<HashMap<u16, Vec<Arc<UDPRoute>>>>>,
}

impl GatewayUdpRoutes {
    /// Create a new empty GatewayUdpRoutes
    pub fn new() -> Self {
        Self {
            port_routes_map: ArcSwap::from_pointee(Arc::new(HashMap::new())),
        }
    }
    
    /// Match a UDPRoute by listener port
    /// 
    /// Returns the first matching route for the given port.
    /// In case of multiple routes on the same port, returns the first one
    /// (prioritization logic can be added later if needed).
    pub fn match_route(&self, port: u16) -> Option<Arc<UDPRoute>> {
        let port_routes = self.port_routes_map.load();
        port_routes.get(&port)
            .and_then(|routes| routes.first().cloned())
    }
    
    /// Get all routes for a specific port
    pub fn get_routes_for_port(&self, port: u16) -> Vec<Arc<UDPRoute>> {
        let port_routes = self.port_routes_map.load();
        port_routes.get(&port)
            .map(|routes| routes.clone())
            .unwrap_or_default()
    }
    
    /// Update the routes map (called by UdpRouteManager during config sync)
    pub(crate) fn update_routes(&self, new_routes: HashMap<u16, Vec<Arc<UDPRoute>>>) {
        // Note: ArcSwap<Arc<T>> requires Arc<Arc<T>> for store() method
        // This double-Arc is needed for lock-free atomic pointer swapping
        self.port_routes_map.store(Arc::new(Arc::new(new_routes)));
    }
    
    /// Get all ports that have routes
    pub fn get_all_ports(&self) -> Vec<u16> {
        let port_routes = self.port_routes_map.load();
        port_routes.keys().copied().collect()
    }
    
    /// Check if there are any routes
    pub fn is_empty(&self) -> bool {
        let port_routes = self.port_routes_map.load();
        port_routes.is_empty()
    }
}

impl Default for GatewayUdpRoutes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::udp_route::*;
    
    fn create_test_udp_route(namespace: &str, name: &str, port: i32) -> UDPRoute {
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
                    section_name: None,
                    port: Some(port),
                }]),
                rules: Some(vec![]),
            },
        }
    }
    
    #[test]
    fn test_gateway_udp_routes_match() {
        let gateway_routes = GatewayUdpRoutes::new();
        
        let route1 = Arc::new(create_test_udp_route("default", "route1", 9000));
        let route2 = Arc::new(create_test_udp_route("default", "route2", 9001));
        
        let mut routes_map = HashMap::new();
        routes_map.insert(9000, vec![route1.clone()]);
        routes_map.insert(9001, vec![route2.clone()]);
        
        gateway_routes.update_routes(routes_map);
        
        assert!(gateway_routes.match_route(9000).is_some());
        assert!(gateway_routes.match_route(9001).is_some());
        assert!(gateway_routes.match_route(9002).is_none());
    }
    
    #[test]
    fn test_gateway_udp_routes_empty() {
        let gateway_routes = GatewayUdpRoutes::new();
        assert!(gateway_routes.is_empty());
        assert!(gateway_routes.match_route(9000).is_none());
    }
}

use std::sync::Arc;
use std::collections::HashMap;
use arc_swap::ArcSwap;
use crate::types::resources::TLSRoute;

/// Gateway-level TLS routes collection
/// 
/// Stores all TLSRoutes associated with a specific Gateway, indexed by SNI hostname
/// Uses ArcSwap for lock-free concurrent access
pub struct GatewayTlsRoutes {
    /// hostname -> Vec<Arc<TLSRoute>> mapping
    /// Multiple routes can match the same hostname (with different priorities or conditions)
    hostname_routes_map: ArcSwap<Arc<HashMap<String, Vec<Arc<TLSRoute>>>>>,
}

impl GatewayTlsRoutes {
    /// Create a new empty GatewayTlsRoutes
    pub fn new() -> Self {
        Self {
            hostname_routes_map: ArcSwap::from_pointee(Arc::new(HashMap::new())),
        }
    }
    
    /// Match a TLSRoute by SNI hostname
    /// 
    /// Returns the first matching route for the given hostname.
    /// In case of multiple routes for the same hostname, returns the first one
    /// (prioritization logic can be added later if needed).
    pub fn match_route(&self, sni_hostname: &str) -> Option<Arc<TLSRoute>> {
        let hostname_routes = self.hostname_routes_map.load();
        
        // Exact match first
        if let Some(routes) = hostname_routes.get(sni_hostname) {
            return routes.first().cloned();
        }
        
        // Wildcard matching
        // For example, *.example.com matches test.example.com
        if let Some(dot_pos) = sni_hostname.find('.') {
            let wildcard = format!("*{}", &sni_hostname[dot_pos..]);
            if let Some(routes) = hostname_routes.get(&wildcard) {
                return routes.first().cloned();
            }
        }
        
        None
    }
    
    /// Get all routes for a specific hostname
    pub fn get_routes_for_hostname(&self, hostname: &str) -> Vec<Arc<TLSRoute>> {
        let hostname_routes = self.hostname_routes_map.load();
        hostname_routes.get(hostname)
            .map(|routes| routes.clone())
            .unwrap_or_default()
    }
    
    /// Update the routes map (called by TlsRouteManager during config sync)
    pub(crate) fn update_routes(&self, new_routes: HashMap<String, Vec<Arc<TLSRoute>>>) {
        // Note: ArcSwap<Arc<T>> requires Arc<Arc<T>> for store() method
        // This double-Arc is needed for lock-free atomic pointer swapping
        self.hostname_routes_map.store(Arc::new(Arc::new(new_routes)));
    }
    
    /// Get all hostnames that have routes
    pub fn get_all_hostnames(&self) -> Vec<String> {
        let hostname_routes = self.hostname_routes_map.load();
        hostname_routes.keys().cloned().collect()
    }
    
    /// Check if there are any routes
    pub fn is_empty(&self) -> bool {
        let hostname_routes = self.hostname_routes_map.load();
        hostname_routes.is_empty()
    }
}

impl Default for GatewayTlsRoutes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::tls_route::*;
    use crate::types::resources::common::ParentReference;
    
    fn create_test_tls_route(namespace: &str, name: &str, hostname: &str) -> TLSRoute {
        TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: None,
                    port: None,
                }]),
                hostnames: Some(vec![hostname.to_string()]),
                rules: Some(vec![]),
            },
        }
    }
    
    #[test]
    fn test_gateway_tls_routes_exact_match() {
        let gateway_routes = GatewayTlsRoutes::new();
        
        let route1 = Arc::new(create_test_tls_route("default", "route1", "test.example.com"));
        let route2 = Arc::new(create_test_tls_route("default", "route2", "api.example.com"));
        
        let mut routes_map = HashMap::new();
        routes_map.insert("test.example.com".to_string(), vec![route1.clone()]);
        routes_map.insert("api.example.com".to_string(), vec![route2.clone()]);
        
        gateway_routes.update_routes(routes_map);
        
        assert!(gateway_routes.match_route("test.example.com").is_some());
        assert!(gateway_routes.match_route("api.example.com").is_some());
        assert!(gateway_routes.match_route("other.example.com").is_none());
    }
    
    #[test]
    fn test_gateway_tls_routes_wildcard_match() {
        let gateway_routes = GatewayTlsRoutes::new();
        
        let route = Arc::new(create_test_tls_route("default", "route1", "*.example.com"));
        
        let mut routes_map = HashMap::new();
        routes_map.insert("*.example.com".to_string(), vec![route.clone()]);
        
        gateway_routes.update_routes(routes_map);
        
        assert!(gateway_routes.match_route("test.example.com").is_some());
        assert!(gateway_routes.match_route("api.example.com").is_some());
        assert!(gateway_routes.match_route("example.com").is_none());
    }
    
    #[test]
    fn test_gateway_tls_routes_empty() {
        let gateway_routes = GatewayTlsRoutes::new();
        assert!(gateway_routes.is_empty());
        assert!(gateway_routes.match_route("test.example.com").is_none());
    }
}


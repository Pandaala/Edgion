use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::udp_routes::{UdpRouteManager, get_global_udp_route_manager};
use crate::types::{UDPRoute, ResourceMeta};

/// Implement ConfHandler for Arc<UdpRouteManager>
impl ConfHandler<UDPRoute> for Arc<UdpRouteManager> {
    fn full_set(&self, data: &HashMap<String, UDPRoute>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, UDPRoute>,
        update: HashMap<String, UDPRoute>,
        remove: HashSet<String>
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Implement ConfHandler for &'static UdpRouteManager
impl ConfHandler<UDPRoute> for &'static UdpRouteManager {
    fn full_set(&self, data: &HashMap<String, UDPRoute>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, UDPRoute>,
        update: HashMap<String, UDPRoute>,
        remove: HashSet<String>
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a UdpRouteManager handler for registration with ConfigClient
pub fn create_udp_route_handler() -> Box<dyn ConfHandler<UDPRoute> + Send + Sync> {
    Box::new(get_global_udp_route_manager())
}

impl ConfHandler<UDPRoute> for UdpRouteManager {
    fn full_set(&self, data: &HashMap<String, UDPRoute>) {
        tracing::info!(
            component = "udp_route_manager",
            cnt = data.len(),
            "full set"
        );
        
        // Initialize all routes
        let mut processed_routes = HashMap::new();
        for (key, route) in data {
            match self.initialize_route(route.clone()) {
                Ok(initialized_route) => {
                    processed_routes.insert(key.clone(), initialized_route);
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to initialize UDPRoute"
                    );
                }
            }
        }
        
        self.replace_all(processed_routes);
    }

    fn partial_update(
        &self,
        add: HashMap<String, UDPRoute>,
        update: HashMap<String, UDPRoute>,
        remove: HashSet<String>
    ) {
        tracing::info!(
            component = "udp_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );
        
        // Process additions
        for (key, route) in add {
            match self.initialize_route(route) {
                Ok(initialized_route) => {
                    self.add_route(initialized_route);
                    tracing::debug!(resource_key = %key, "Added UDPRoute");
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to add UDPRoute"
                    );
                }
            }
        }
        
        // Process updates
        for (key, route) in update {
            match self.initialize_route(route) {
                Ok(initialized_route) => {
                    // Remove old version first
                    self.remove_route(&key);
                    // Add new version
                    self.add_route(initialized_route);
                    tracing::debug!(resource_key = %key, "Updated UDPRoute");
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to update UDPRoute"
                    );
                }
            }
        }
        
        // Process removals
        for key in remove {
            self.remove_route(&key);
            tracing::debug!(resource_key = %key, "Removed UDPRoute");
        }
    }
}

impl UdpRouteManager {
    /// Initialize a UDPRoute by setting up BackendSelector
    fn initialize_route(&self, mut route: UDPRoute) -> Result<Arc<UDPRoute>, String> {
        let route_key = route.key_name();
        
        // Initialize rules
        if let Some(rules) = route.spec.rules.as_mut() {
            for (rule_idx, rule) in rules.iter_mut().enumerate() {
                // Initialize BackendSelector
                if let Some(backend_refs) = &rule.backend_refs {
                    let backends: Vec<_> = backend_refs.iter().cloned().collect();
                    let weights: Vec<_> = backend_refs.iter()
                        .map(|br| br.weight)
                        .collect();
                    
                    rule.backend_finder.init(backends, weights);
                    
                    tracing::debug!(
                        route = %route_key,
                        rule_idx,
                        backend_count = backend_refs.len(),
                        "Initialized BackendSelector for UDPRoute rule"
                    );
                }
            }
        }
        
        Ok(Arc::new(route))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ResourceMeta;
    use crate::types::resources::common::ParentReference;
    
    fn create_test_udp_route(namespace: &str, name: &str, gateway: &str, port: i32) -> UDPRoute {
        use crate::types::resources::udp_route::*;
        
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
                    name: gateway.to_string(),
                    section_name: None,
                    port: Some(port),
                }]),
                rules: Some(vec![UDPRouteRule {
                    backend_refs: Some(vec![UDPBackendRef {
                        name: "test-service".to_string(),
                        namespace: Some(namespace.to_string()),
                        port: Some(8080),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        extension_info: Default::default(),
                        plugin_runtime: Default::default(),
                    }]),
                    stream_plugin_runtime: Default::default(),
                    backend_finder: Default::default(),
                    plugin_runtime: Default::default(),
                }]),
            },
        }
    }
    
    #[test]
    fn test_udp_route_full_set() {
        let manager = UdpRouteManager::new();
        
        let mut data = HashMap::new();
        let route1 = create_test_udp_route("default", "route1", "gateway1", 9000);
        let route2 = create_test_udp_route("default", "route2", "gateway1", 9001);
        
        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);
        
        manager.full_set(&data);
        
        // Test via GatewayUdpRoutes
        let gateway_routes = manager.get_or_create_gateway_udp_routes("default", "gateway1");
        assert!(gateway_routes.match_route(9000).is_some());
        assert!(gateway_routes.match_route(9001).is_some());
        assert!(gateway_routes.match_route(9002).is_none());
    }
    
    #[test]
    fn test_udp_route_partial_update() {
        let manager = UdpRouteManager::new();
        
        // Add a route
        let mut add = HashMap::new();
        let route1 = create_test_udp_route("default", "route1", "gateway1", 9000);
        add.insert("default/route1".to_string(), route1);
        
        manager.partial_update(add, HashMap::new(), HashSet::new());
        
        // Test via GatewayUdpRoutes
        let gateway_routes = manager.get_or_create_gateway_udp_routes("default", "gateway1");
        assert!(gateway_routes.match_route(9000).is_some());
        
        // Remove the route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        manager.partial_update(HashMap::new(), HashMap::new(), remove);
        assert!(gateway_routes.match_route(9000).is_none());
    }
}


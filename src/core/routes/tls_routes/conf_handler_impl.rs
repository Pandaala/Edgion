use crate::core::conf_sync::traits::ConfHandler;
use crate::core::routes::tls_routes::{get_global_tls_route_manager, TlsRouteManager};
use crate::types::{ResourceMeta, TLSRoute};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Implement ConfHandler for Arc<TlsRouteManager>
impl ConfHandler<TLSRoute> for Arc<TlsRouteManager> {
    fn full_set(&self, data: &HashMap<String, TLSRoute>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, TLSRoute>,
        update: HashMap<String, TLSRoute>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Implement ConfHandler for &'static TlsRouteManager
impl ConfHandler<TLSRoute> for &'static TlsRouteManager {
    fn full_set(&self, data: &HashMap<String, TLSRoute>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, TLSRoute>,
        update: HashMap<String, TLSRoute>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a TlsRouteManager handler for registration with ConfigClient
pub fn create_tls_route_handler() -> Box<dyn ConfHandler<TLSRoute> + Send + Sync> {
    Box::new(get_global_tls_route_manager())
}

impl ConfHandler<TLSRoute> for TlsRouteManager {
    fn full_set(&self, data: &HashMap<String, TLSRoute>) {
        tracing::info!(component = "tls_route_manager", cnt = data.len(), "full set");

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
                        "Failed to initialize TLSRoute"
                    );
                }
            }
        }

        self.replace_all(processed_routes);
    }

    fn partial_update(
        &self,
        add: HashMap<String, TLSRoute>,
        update: HashMap<String, TLSRoute>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "tls_route_manager",
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
                    tracing::debug!(resource_key = %key, "Added TLSRoute");
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to add TLSRoute"
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
                    tracing::debug!(resource_key = %key, "Updated TLSRoute");
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to update TLSRoute"
                    );
                }
            }
        }

        // Process removals
        for key in remove {
            self.remove_route(&key);
            tracing::debug!(resource_key = %key, "Removed TLSRoute");
        }
    }
}

impl TlsRouteManager {
    /// Initialize a TLSRoute by setting up BackendSelector
    fn initialize_route(&self, mut route: TLSRoute) -> Result<Arc<TLSRoute>, String> {
        let route_key = route.key_name();

        // Initialize rules
        if let Some(rules) = route.spec.rules.as_mut() {
            for (rule_idx, rule) in rules.iter_mut().enumerate() {
                // Initialize BackendSelector
                if let Some(backend_refs) = &rule.backend_refs {
                    let backends: Vec<_> = backend_refs.to_vec();
                    let weights: Vec<_> = backend_refs.iter().map(|br| br.weight).collect();

                    rule.backend_finder.init(backends, weights);

                    tracing::debug!(
                        route = %route_key,
                        rule_idx,
                        backend_count = backend_refs.len(),
                        "Initialized BackendSelector for TLSRoute rule"
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
    use crate::types::resources::common::ParentReference;

    fn create_test_tls_route(namespace: &str, name: &str, gateway: &str, hostname: &str) -> TLSRoute {
        use crate::types::resources::tls_route::*;

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
                    name: gateway.to_string(),
                    section_name: None,
                    port: None,
                }]),
                hostnames: Some(vec![hostname.to_string()]),
                rules: Some(vec![TLSRouteRule {
                    backend_refs: Some(vec![TLSBackendRef {
                        name: "test-service".to_string(),
                        namespace: Some(namespace.to_string()),
                        port: Some(8080),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        extension_info: Default::default(),
                        plugin_runtime: Default::default(),
                    }]),
                    backend_finder: Default::default(),
                    plugin_runtime: Default::default(),
                }]),
            },
            status: None,
        }
    }

    #[test]
    fn test_tls_route_full_set() {
        let manager = TlsRouteManager::new();

        let mut data = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com");
        let route2 = create_test_tls_route("default", "route2", "gateway1", "api.example.com");

        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        manager.full_set(&data);

        // Test via GatewayTlsRoutes
        let gateway_routes = manager.get_or_create_gateway_tls_routes("default", "gateway1");
        assert!(gateway_routes.match_route("test.example.com").is_some());
        assert!(gateway_routes.match_route("api.example.com").is_some());
        assert!(gateway_routes.match_route("other.example.com").is_none());
    }

    #[test]
    fn test_tls_route_partial_update() {
        let manager = TlsRouteManager::new();

        // Add a route
        let mut add = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com");
        add.insert("default/route1".to_string(), route1);

        manager.partial_update(add, HashMap::new(), HashSet::new());

        // Test via GatewayTlsRoutes
        let gateway_routes = manager.get_or_create_gateway_tls_routes("default", "gateway1");
        assert!(gateway_routes.match_route("test.example.com").is_some());

        // Remove the route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        manager.partial_update(HashMap::new(), HashMap::new(), remove);
        assert!(gateway_routes.match_route("test.example.com").is_none());
    }
}

use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::routes::udp::routes_mgr::{
    get_global_udp_route_managers, resolved_ports_for_route, GlobalUdpRouteManagers,
};
use crate::types::{ResourceMeta, UDPRoute};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Implement ConfHandler for &'static GlobalUdpRouteManagers
impl ConfHandler<UDPRoute> for &'static GlobalUdpRouteManagers {
    fn full_set(&self, data: &HashMap<String, UDPRoute>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, UDPRoute>,
        update: HashMap<String, UDPRoute>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a handler for registration with ConfigClient.
pub fn create_udp_route_handler() -> Box<dyn ConfHandler<UDPRoute> + Send + Sync> {
    Box::new(get_global_udp_route_managers())
}

impl ConfHandler<UDPRoute> for GlobalUdpRouteManagers {
    fn full_set(&self, data: &HashMap<String, UDPRoute>) {
        tracing::info!(component = "udp_route_manager", cnt = data.len(), "full set");

        self.clear_route_cache();

        for (key, route) in data {
            match self.initialize_route(route.clone()) {
                Ok(initialized) => {
                    self.insert_route(initialized);
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to initialize UDPRoute during full_set"
                    );
                }
            }
        }

        self.rebuild_all_port_managers();
    }

    fn partial_update(
        &self,
        add: HashMap<String, UDPRoute>,
        update: HashMap<String, UDPRoute>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "udp_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut affected_ports: HashSet<u16> = HashSet::new();

        // Collect old ports from routes being removed or updated BEFORE modifying the cache
        for key in remove.iter().chain(update.keys()) {
            if let Some(existing) = self.get_route(key) {
                for &port in resolved_ports_for_route(&existing) {
                    affected_ports.insert(port);
                }
            }
        }

        // Process removals
        for key in &remove {
            self.remove_route(key);
            tracing::debug!(resource_key = %key, "Removed UDPRoute");
        }

        // Process updates (remove old, add new)
        for (key, route) in update {
            self.remove_route(&key);
            match self.initialize_route(route) {
                Ok(initialized) => {
                    for &port in resolved_ports_for_route(&initialized) {
                        affected_ports.insert(port);
                    }
                    self.insert_route(initialized);
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

        // Process additions
        for (key, route) in add {
            match self.initialize_route(route) {
                Ok(initialized) => {
                    for &port in resolved_ports_for_route(&initialized) {
                        affected_ports.insert(port);
                    }
                    self.insert_route(initialized);
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

        self.rebuild_affected_port_managers(&affected_ports);
    }
}

impl GlobalUdpRouteManagers {
    /// Initialize a UDPRoute by setting up BackendSelector for each rule.
    fn initialize_route(&self, mut route: UDPRoute) -> Result<Arc<UDPRoute>, String> {
        let route_key = route.key_name();

        if let Some(rules) = route.spec.rules.as_mut() {
            for (rule_idx, rule) in rules.iter_mut().enumerate() {
                if let Some(backend_refs) = &rule.backend_refs {
                    let backends: Vec<_> = backend_refs.to_vec();
                    let weights: Vec<_> = backend_refs.iter().map(|br| br.weight).collect();

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
    use crate::core::common::conf_sync::traits::ConfHandler;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::udp_route::*;

    fn create_test_udp_route(namespace: &str, name: &str, gateway: &str, listener_name: &str, port: u16) -> UDPRoute {
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
                    section_name: Some(listener_name.to_string()),
                    port: Some(port as i32),
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
                        ref_denied: None,
                    }]),
                    stream_plugin_runtime: Default::default(),
                    backend_finder: Default::default(),
                    plugin_runtime: Default::default(),
                }]),
                resolved_ports: Some(vec![port]),
            },
            status: None,
        }
    }

    #[test]
    fn test_udp_route_full_set() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);
        let _ = managers.get_or_create_port_manager(9001);

        let mut data = HashMap::new();
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000);
        let route2 = create_test_udp_route("default", "route2", "gateway1", "udp-9001", 9001);

        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        managers.full_set(&data);

        let mgr9000 = managers.get_or_create_port_manager(9000);
        assert!(mgr9000.load_route_table().match_route().is_some());

        let mgr9001 = managers.get_or_create_port_manager(9001);
        assert!(mgr9001.load_route_table().match_route().is_some());

        let mgr9002 = managers.get_or_create_port_manager(9002);
        assert!(mgr9002.load_route_table().match_route().is_none());
    }

    #[test]
    fn test_udp_route_partial_update_add_remove() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);

        // Add a route
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000);
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), route1);
        managers.partial_update(add, HashMap::new(), HashSet::new());

        let mgr = managers.get_or_create_port_manager(9000);
        assert!(mgr.load_route_table().match_route().is_some());

        // Remove the route
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        managers.partial_update(HashMap::new(), HashMap::new(), remove);

        assert!(mgr.load_route_table().match_route().is_none());
    }

    #[test]
    fn test_udp_route_partial_update_update() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);
        let _ = managers.get_or_create_port_manager(9001);

        // Add initial route on port 9000
        let route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000);
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), route);
        managers.partial_update(add, HashMap::new(), HashSet::new());

        assert!(managers
            .get_or_create_port_manager(9000)
            .load_route_table()
            .match_route()
            .is_some());

        // Update route to port 9001
        let updated_route = create_test_udp_route("default", "route1", "gateway1", "udp-9001", 9001);
        let mut update = HashMap::new();
        update.insert("default/route1".to_string(), updated_route);
        managers.partial_update(HashMap::new(), update, HashSet::new());

        // Port 9000 should be empty, port 9001 should have the route
        assert!(managers
            .get_or_create_port_manager(9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert!(managers
            .get_or_create_port_manager(9001)
            .load_route_table()
            .match_route()
            .is_some());
    }

    #[test]
    fn test_full_set_clears_previous_state() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);

        // First full_set
        let mut data = HashMap::new();
        data.insert(
            "default/route1".to_string(),
            create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000),
        );
        managers.full_set(&data);
        assert!(managers
            .get_or_create_port_manager(9000)
            .load_route_table()
            .match_route()
            .is_some());

        // Second full_set with empty data should clear everything
        managers.full_set(&HashMap::new());
        assert!(managers
            .get_or_create_port_manager(9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert_eq!(managers.stats().route_cache, 0);
    }

    #[test]
    fn test_route_without_resolved_ports_skipped() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);

        // Create a route without resolved_ports
        let mut route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000);
        route.spec.resolved_ports = None;

        let mut data = HashMap::new();
        data.insert("default/route1".to_string(), route);
        managers.full_set(&data);

        // Route is in cache but not assigned to any port
        assert_eq!(managers.stats().route_cache, 1);
        assert!(managers
            .get_or_create_port_manager(9000)
            .load_route_table()
            .match_route()
            .is_none());
    }

    #[test]
    fn test_multi_port_route() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);
        let _ = managers.get_or_create_port_manager(9001);

        // Create a route that resolves to multiple ports
        let mut route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000);
        route.spec.resolved_ports = Some(vec![9000, 9001]);

        let mut data = HashMap::new();
        data.insert("default/route1".to_string(), route);
        managers.full_set(&data);

        assert!(managers
            .get_or_create_port_manager(9000)
            .load_route_table()
            .match_route()
            .is_some());
        assert!(managers
            .get_or_create_port_manager(9001)
            .load_route_table()
            .match_route()
            .is_some());
    }

    #[test]
    fn test_partial_update_concurrent_add_and_remove() {
        let managers = GlobalUdpRouteManagers::new();
        let _ = managers.get_or_create_port_manager(9000);
        let _ = managers.get_or_create_port_manager(9001);

        // Add initial route
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000);
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), route1);
        managers.partial_update(add, HashMap::new(), HashSet::new());

        // Simultaneously add route2 on 9001 and remove route1 on 9000
        let route2 = create_test_udp_route("default", "route2", "gateway1", "udp-9001", 9001);
        let mut add = HashMap::new();
        add.insert("default/route2".to_string(), route2);
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());

        managers.partial_update(add, HashMap::new(), remove);

        assert!(managers
            .get_or_create_port_manager(9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert!(managers
            .get_or_create_port_manager(9001)
            .load_route_table()
            .match_route()
            .is_some());
        assert_eq!(managers.stats().route_cache, 1);
    }
}

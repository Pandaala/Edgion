use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::routes::l4_partial_update::apply_l4_partial_update;
use crate::core::gateway::routes::udp::routes_mgr::{
    get_global_udp_route_managers, resolved_listeners_for_route, GlobalUdpRouteManagers,
};
use crate::types::{ResourceMeta, UDPRoute};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

        self.rebuild_all_listener_managers();
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

        apply_l4_partial_update(
            "UDPRoute",
            add,
            update,
            remove,
            |key| self.get_route(key),
            |r| resolved_listeners_for_route(r).to_vec(),
            |key| self.remove_route(key),
            |route| {
                self.initialize_route(route).map(|initialized| {
                    let lks = resolved_listeners_for_route(&initialized).to_vec();
                    self.insert_route(initialized);
                    lks
                })
            },
            |affected| self.rebuild_affected_listener_managers(affected),
        );
    }
}

impl GlobalUdpRouteManagers {
    /// Initialize a UDPRoute by setting up BackendSelector for each rule.
    fn initialize_route(&self, mut route: UDPRoute) -> Result<Arc<UDPRoute>, String> {
        let route_key = route.key_name();

        if let Some(rules) = route.spec.rules.as_mut() {
            for (rule_idx, rule) in rules.iter_mut().enumerate() {
                if let Some(backend_refs) = &rule.backend_refs {
                    let (backends, weights): (Vec<_>, Vec<_>) = backend_refs
                        .iter()
                        .filter(|br| is_service_backend_ref(br))
                        .map(|br| (br.clone(), br.weight))
                        .unzip();

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

/// A BackendRef is routable only when it targets a core Service
/// (group is "" or "core" and kind is "Service" or omitted).
fn is_service_backend_ref(br: &crate::types::resources::udp_route::UDPBackendRef) -> bool {
    let group_ok = match br.group.as_deref() {
        None | Some("") | Some("core") => true,
        _ => false,
    };
    let kind_ok = match br.kind.as_deref() {
        None | Some("Service") => true,
        _ => false,
    };
    group_ok && kind_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::conf_sync::traits::ConfHandler;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::udp_route::*;
    use crate::types::ListenerKey;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_udp_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        listener_name: &str,
        port: u16,
        listener_key: ListenerKey,
    ) -> UDPRoute {
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
                resolved_listeners: Some(vec![listener_key]),
            },
            status: None,
        }
    }

    #[test]
    fn test_udp_route_full_set() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        let mut data = HashMap::new();
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let route2 = create_test_udp_route("default", "route2", "gateway1", "udp-9001", 9001, key9001.clone());

        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        managers.full_set(&data);

        let mgr9000 = managers.get_or_create_listener_manager(&key9000);
        assert!(mgr9000.load_route_table().match_route().is_some());

        let mgr9001 = managers.get_or_create_listener_manager(&key9001);
        assert!(mgr9001.load_route_table().match_route().is_some());

        let key9002 = make_listener_key(9002, "UDP");
        let mgr9002 = managers.get_or_create_listener_manager(&key9002);
        assert!(mgr9002.load_route_table().match_route().is_none());
    }

    #[test]
    fn test_udp_route_partial_update_add_remove() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);

        // Add a route
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), route1);
        managers.partial_update(add, HashMap::new(), HashSet::new());

        let mgr = managers.get_or_create_listener_manager(&key9000);
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
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        // Add initial route on listener 9000
        let route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), route);
        managers.partial_update(add, HashMap::new(), HashSet::new());

        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_some());

        // Update route to listener 9001
        let updated_route = create_test_udp_route("default", "route1", "gateway1", "udp-9001", 9001, key9001.clone());
        let mut update = HashMap::new();
        update.insert("default/route1".to_string(), updated_route);
        managers.partial_update(HashMap::new(), update, HashSet::new());

        // Listener 9000 should be empty, listener 9001 should have the route
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert!(managers
            .get_or_create_listener_manager(&key9001)
            .load_route_table()
            .match_route()
            .is_some());
    }

    #[test]
    fn test_full_set_clears_previous_state() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);

        // First full_set
        let mut data = HashMap::new();
        data.insert(
            "default/route1".to_string(),
            create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone()),
        );
        managers.full_set(&data);
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_some());

        // Second full_set with empty data should clear everything
        managers.full_set(&HashMap::new());
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert_eq!(managers.stats().route_cache, 0);
    }

    #[test]
    fn test_route_without_resolved_listeners_skipped() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);

        // Create a route without resolved_listeners
        let mut route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        route.spec.resolved_listeners = None;

        let mut data = HashMap::new();
        data.insert("default/route1".to_string(), route);
        managers.full_set(&data);

        // Route is in cache but not assigned to any listener
        assert_eq!(managers.stats().route_cache, 1);
        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
    }

    #[test]
    fn test_multi_listener_route() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        // Create a route that resolves to multiple listeners
        let mut route = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        route.spec.resolved_listeners = Some(vec![key9000.clone(), key9001.clone()]);

        let mut data = HashMap::new();
        data.insert("default/route1".to_string(), route);
        managers.full_set(&data);

        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_some());
        assert!(managers
            .get_or_create_listener_manager(&key9001)
            .load_route_table()
            .match_route()
            .is_some());
    }

    #[test]
    fn test_partial_update_concurrent_add_and_remove() {
        let managers = GlobalUdpRouteManagers::new();
        let key9000 = make_listener_key(9000, "UDP");
        let key9001 = make_listener_key(9001, "UDP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        // Add initial route
        let route1 = create_test_udp_route("default", "route1", "gateway1", "udp-9000", 9000, key9000.clone());
        let mut add = HashMap::new();
        add.insert("default/route1".to_string(), route1);
        managers.partial_update(add, HashMap::new(), HashSet::new());

        // Simultaneously add route2 on 9001 and remove route1 on 9000
        let route2 = create_test_udp_route("default", "route2", "gateway1", "udp-9001", 9001, key9001.clone());
        let mut add = HashMap::new();
        add.insert("default/route2".to_string(), route2);
        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());

        managers.partial_update(add, HashMap::new(), remove);

        assert!(managers
            .get_or_create_listener_manager(&key9000)
            .load_route_table()
            .match_route()
            .is_none());
        assert!(managers
            .get_or_create_listener_manager(&key9001)
            .load_route_table()
            .match_route()
            .is_some());
        assert_eq!(managers.stats().route_cache, 1);
    }

    #[test]
    fn test_is_service_backend_ref_filters_unknown_kind() {
        let valid = UDPBackendRef {
            name: "svc".to_string(),
            namespace: None,
            port: Some(80),
            weight: Some(1),
            group: None,
            kind: None,
            extension_info: Default::default(),
            plugin_runtime: Default::default(),
            ref_denied: None,
        };
        assert!(is_service_backend_ref(&valid));

        let explicit_service = UDPBackendRef {
            group: Some("core".to_string()),
            kind: Some("Service".to_string()),
            ..valid.clone()
        };
        assert!(is_service_backend_ref(&explicit_service));

        let unknown_kind = UDPBackendRef {
            group: Some("example.com".to_string()),
            kind: Some("Foo".to_string()),
            ..valid
        };
        assert!(!is_service_backend_ref(&unknown_kind));
    }
}

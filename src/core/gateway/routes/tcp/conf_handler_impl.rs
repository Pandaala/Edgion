use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::routes::tcp::routes_mgr::{
    get_global_tcp_route_managers, resolved_listeners_for_route, GlobalTcpRouteManagers,
};
use crate::types::{ListenerKey, ResourceMeta, TCPRoute};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Implement ConfHandler for &'static GlobalTcpRouteManagers
impl ConfHandler<TCPRoute> for &'static GlobalTcpRouteManagers {
    fn full_set(&self, data: &HashMap<String, TCPRoute>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, TCPRoute>,
        update: HashMap<String, TCPRoute>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a handler for registration with ConfigClient.
pub fn create_tcp_route_handler() -> Box<dyn ConfHandler<TCPRoute> + Send + Sync> {
    Box::new(get_global_tcp_route_managers())
}

impl ConfHandler<TCPRoute> for GlobalTcpRouteManagers {
    fn full_set(&self, data: &HashMap<String, TCPRoute>) {
        tracing::info!(component = "tcp_route_manager", cnt = data.len(), "full set");

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
                        "Failed to initialize TCPRoute during full_set"
                    );
                }
            }
        }

        self.rebuild_all_listener_managers();
    }

    fn partial_update(
        &self,
        add: HashMap<String, TCPRoute>,
        update: HashMap<String, TCPRoute>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "tcp_route_manager",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();

        // Collect old listeners from routes being removed or updated BEFORE modifying the cache
        for key in remove.iter().chain(update.keys()) {
            if let Some(existing) = self.get_route(key) {
                for lk in resolved_listeners_for_route(&existing) {
                    affected_listeners.insert(lk.clone());
                }
            }
        }

        // Process removals
        for key in &remove {
            self.remove_route(key);
            tracing::debug!(resource_key = %key, "Removed TCPRoute");
        }

        // Process updates (remove old, add new)
        for (key, route) in update {
            self.remove_route(&key);
            match self.initialize_route(route) {
                Ok(initialized) => {
                    for lk in resolved_listeners_for_route(&initialized) {
                        affected_listeners.insert(lk.clone());
                    }
                    self.insert_route(initialized);
                    tracing::debug!(resource_key = %key, "Updated TCPRoute");
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to update TCPRoute"
                    );
                }
            }
        }

        // Process additions
        for (key, route) in add {
            match self.initialize_route(route) {
                Ok(initialized) => {
                    for lk in resolved_listeners_for_route(&initialized) {
                        affected_listeners.insert(lk.clone());
                    }
                    self.insert_route(initialized);
                    tracing::debug!(resource_key = %key, "Added TCPRoute");
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to add TCPRoute"
                    );
                }
            }
        }

        self.rebuild_affected_listener_managers(&affected_listeners);
    }
}

/// Annotation key for referencing EdgionStreamPlugins from TCPRoute.
/// Same annotation as Gateway-level: `edgion.io/edgion-stream-plugins`.
/// Value format: "namespace/name" or just "name" (namespace inferred from TCPRoute).
const ANNOTATION_EDGION_STREAM_PLUGINS: &str = "edgion.io/edgion-stream-plugins";

impl GlobalTcpRouteManagers {
    /// Initialize a TCPRoute by setting up BackendSelector and stream plugin store key.
    fn initialize_route(&self, mut route: TCPRoute) -> Result<Arc<TCPRoute>, String> {
        let route_key = route.key_name();

        let store_key = Self::resolve_stream_plugin_store_key(&route);

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
                        "Initialized BackendSelector for TCPRoute rule"
                    );
                }

                if let Some(ref key) = store_key {
                    rule.stream_plugin_store_key = Some(key.clone());
                    tracing::info!(
                        route = %route_key,
                        rule_idx,
                        store_key = %key,
                        "Set stream plugin store key for TCPRoute rule (dynamic lookup)"
                    );
                }
            }
        }

        Ok(Arc::new(route))
    }

    /// Resolve the stream plugin store key from the TCPRoute's annotation.
    fn resolve_stream_plugin_store_key(route: &TCPRoute) -> Option<String> {
        let annotations = route.metadata.annotations.as_ref()?;
        let annotation_value = annotations.get(ANNOTATION_EDGION_STREAM_PLUGINS)?;
        let trimmed = annotation_value.trim();
        if trimmed.is_empty() {
            return None;
        }

        let store_key = if trimmed.contains('/') {
            trimmed.to_string()
        } else {
            let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
            format!("{}/{}", namespace, trimmed)
        };

        Some(store_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::conf_sync::traits::ConfHandler;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::tcp_route::*;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_tcp_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        listener_name: &str,
        port: u16,
        listener_key: ListenerKey,
    ) -> TCPRoute {
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
                    name: gateway.to_string(),
                    section_name: Some(listener_name.to_string()),
                    port: Some(port as i32),
                }]),
                rules: Some(vec![TCPRouteRule {
                    backend_refs: Some(vec![TCPBackendRef {
                        name: "test-service".to_string(),
                        namespace: Some(namespace.to_string()),
                        port: Some(8080),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        ref_denied: None,
                    }]),
                    stream_plugin_runtime: Default::default(),
                    stream_plugin_store_key: None,
                    backend_finder: Default::default(),
                }]),
                resolved_listeners: Some(vec![listener_key]),
            },
            status: None,
        }
    }

    #[test]
    fn test_tcp_route_full_set() {
        let managers = GlobalTcpRouteManagers::new();
        let key9000 = make_listener_key(9000, "TCP");
        let key9001 = make_listener_key(9001, "TCP");
        let _ = managers.get_or_create_listener_manager(&key9000);
        let _ = managers.get_or_create_listener_manager(&key9001);

        let mut data = HashMap::new();
        let route1 = create_test_tcp_route("default", "route1", "gateway1", "tcp-9000", 9000, key9000.clone());
        let route2 = create_test_tcp_route("default", "route2", "gateway1", "tcp-9001", 9001, key9001.clone());

        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        managers.full_set(&data);

        let mgr9000 = managers.get_or_create_listener_manager(&key9000);
        assert!(mgr9000.load_route_table().match_route().is_some());

        let mgr9001 = managers.get_or_create_listener_manager(&key9001);
        assert!(mgr9001.load_route_table().match_route().is_some());

        let key9002 = make_listener_key(9002, "TCP");
        let mgr9002 = managers.get_or_create_listener_manager(&key9002);
        assert!(mgr9002.load_route_table().match_route().is_none());
    }
}

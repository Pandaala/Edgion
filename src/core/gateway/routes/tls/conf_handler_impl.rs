use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::routes::tls::routes_mgr::resolved_ports_for_route;
use crate::core::gateway::routes::tls::{get_global_tls_route_managers, GlobalTlsRouteManagers};
use crate::types::{ResourceMeta, TLSRoute};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

impl ConfHandler<TLSRoute> for &'static GlobalTlsRouteManagers {
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

/// Create a TLS route handler for registration with ConfigClient.
pub fn create_tls_route_handler() -> Box<dyn ConfHandler<TLSRoute> + Send + Sync> {
    Box::new(get_global_tls_route_managers())
}

impl ConfHandler<TLSRoute> for GlobalTlsRouteManagers {
    fn full_set(&self, data: &HashMap<String, TLSRoute>) {
        tracing::info!(
            component = "global_tls_route_managers",
            cnt = data.len(),
            "full set"
        );

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
                        "Failed to initialize TLSRoute during full_set"
                    );
                }
            }
        }

        self.rebuild_all_port_managers();
    }

    fn partial_update(
        &self,
        add: HashMap<String, TLSRoute>,
        update: HashMap<String, TLSRoute>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "global_tls_route_managers",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut affected_ports: HashSet<u16> = HashSet::new();

        // Collect old ports from routes being removed or updated BEFORE modifying the cache
        for key in remove.iter().chain(update.keys()) {
            if let Some(entry) = self.get_route(key) {
                for &port in resolved_ports_for_route(&entry) {
                    affected_ports.insert(port);
                }
            }
        }

        for (key, route) in add {
            match self.initialize_route(route) {
                Ok(initialized) => {
                    for &port in resolved_ports_for_route(&initialized) {
                        affected_ports.insert(port);
                    }
                    self.insert_route(initialized);
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

        for (key, route) in update {
            self.remove_route(&key);
            match self.initialize_route(route) {
                Ok(initialized) => {
                    for &port in resolved_ports_for_route(&initialized) {
                        affected_ports.insert(port);
                    }
                    self.insert_route(initialized);
                    tracing::debug!(resource_key = %key, "Updated TLSRoute");
                }
                Err(e) => {
                    tracing::error!(
                        resource_key = %key,
                        error = %e,
                        "Failed to update TLSRoute, old version removed"
                    );
                }
            }
        }

        for key in remove {
            self.remove_route(&key);
            tracing::debug!(resource_key = %key, "Removed TLSRoute");
        }

        self.rebuild_affected_port_managers(&affected_ports);
    }
}

/// Annotation key for referencing EdgionStreamPlugins from TLSRoute.
const ANNOTATION_EDGION_STREAM_PLUGINS: &str = "edgion.io/edgion-stream-plugins";

impl GlobalTlsRouteManagers {
    /// Initialize a TLSRoute by setting up BackendSelector, proxy protocol,
    /// upstream TLS, and stream plugin store key from annotations.
    fn initialize_route(&self, mut route: TLSRoute) -> Result<Arc<TLSRoute>, String> {
        let route_key = route.key_name();
        let annotations = route.metadata.annotations.as_ref();

        let proxy_protocol_version = annotations
            .and_then(|a| a.get(crate::types::constants::annotations::edgion::PROXY_PROTOCOL))
            .and_then(|v| match v.trim() {
                "v2" => Some(2u8),
                _ => None,
            });

        let upstream_tls = annotations
            .and_then(|a| a.get(crate::types::constants::annotations::edgion::UPSTREAM_TLS))
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let store_key = Self::resolve_stream_plugin_store_key(&route);

        let max_connect_retries = annotations
            .and_then(|a| a.get(crate::types::constants::annotations::edgion::MAX_CONNECT_RETRIES))
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|v| v.max(1))
            .unwrap_or(1);

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
                        "Initialized BackendSelector for TLSRoute rule"
                    );
                }

                rule.proxy_protocol_version = proxy_protocol_version;
                rule.upstream_tls = upstream_tls;
                rule.max_connect_retries = max_connect_retries;

                if let Some(ref key) = store_key {
                    rule.stream_plugin_store_key = Some(key.clone());
                    tracing::info!(
                        route = %route_key,
                        rule_idx,
                        store_key = %key,
                        "Set stream plugin store key for TLSRoute rule (dynamic lookup)"
                    );
                }
            }
        }

        if proxy_protocol_version.is_some() || upstream_tls || max_connect_retries > 1 {
            tracing::info!(
                route = %route_key,
                proxy_protocol = ?proxy_protocol_version,
                upstream_tls,
                max_connect_retries,
                "TLSRoute configured with extended options"
            );
        }

        Ok(Arc::new(route))
    }

    fn resolve_stream_plugin_store_key(route: &TLSRoute) -> Option<String> {
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
                    port: Some(443),
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
                        ref_denied: None,
                    }]),
                    backend_finder: Default::default(),
                    plugin_runtime: Default::default(),
                    proxy_protocol_version: None,
                    upstream_tls: false,
                    stream_plugin_store_key: None,
                    max_connect_retries: 1,
                }]),
                resolved_ports: Some(vec![443]),
            },
            status: None,
        }
    }

    fn create_annotated_tls_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        hostname: &str,
        annotations: std::collections::BTreeMap<String, String>,
    ) -> TLSRoute {
        use crate::types::resources::tls_route::*;

        TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: gateway.to_string(),
                    section_name: None,
                    port: Some(443),
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
                        ref_denied: None,
                    }]),
                    backend_finder: Default::default(),
                    plugin_runtime: Default::default(),
                    proxy_protocol_version: None,
                    upstream_tls: false,
                    stream_plugin_store_key: None,
                    max_connect_retries: 1,
                }]),
                resolved_ports: Some(vec![443]),
            },
            status: None,
        }
    }

    #[test]
    fn test_tls_route_full_set() {
        let managers = GlobalTlsRouteManagers::new();

        let mut data = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com");
        let route2 = create_test_tls_route("default", "route2", "gateway1", "api.example.com");

        data.insert("default/route1".to_string(), route1);
        data.insert("default/route2".to_string(), route2);

        managers.full_set(&data);

        let mgr = managers.get_or_create_port_manager(443);
        let table = mgr.load_route_table();
        assert!(table.match_route("test.example.com").is_some());
        assert!(table.match_route("api.example.com").is_some());
        assert!(table.match_route("other.example.com").is_none());
    }

    #[test]
    fn test_tls_route_partial_update() {
        let managers = GlobalTlsRouteManagers::new();

        let mut add = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com");
        add.insert("default/route1".to_string(), route1);

        managers.partial_update(add, HashMap::new(), HashSet::new());

        let mgr = managers.get_or_create_port_manager(443);
        let table = mgr.load_route_table();
        assert!(table.match_route("test.example.com").is_some());

        let mut remove = HashSet::new();
        remove.insert("default/route1".to_string());
        managers.partial_update(HashMap::new(), HashMap::new(), remove);

        let table = mgr.load_route_table();
        assert!(table.match_route("test.example.com").is_none());
    }

    #[test]
    fn test_proxy_protocol_annotation_v2() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/proxy-protocol".to_string(), "v2".to_string());

        let route = create_annotated_tls_route("default", "pp2-route", "gw1", "*.sandbox.com", annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, Some(2));
        assert!(!rule.upstream_tls);
        assert!(rule.stream_plugin_store_key.is_none());
    }

    #[test]
    fn test_proxy_protocol_annotation_invalid() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/proxy-protocol".to_string(), "v1".to_string());

        let route = create_annotated_tls_route("default", "pp1-route", "gw1", "*.sandbox.com", annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, None);
    }

    #[test]
    fn test_upstream_tls_annotation() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/upstream-tls".to_string(), "true".to_string());

        let route = create_annotated_tls_route("default", "tls-up", "gw1", "test.com", annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert!(rule.upstream_tls);
    }

    #[test]
    fn test_upstream_tls_annotation_false() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/upstream-tls".to_string(), "false".to_string());

        let route = create_annotated_tls_route("default", "tls-up-false", "gw1", "test.com", annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert!(!rule.upstream_tls);
    }

    #[test]
    fn test_stream_plugin_store_key_full_path() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert(
            "edgion.io/edgion-stream-plugins".to_string(),
            "sandbox/sandbox-stream-plugins".to_string(),
        );

        let route = create_annotated_tls_route("default", "sp-route", "gw1", "test.com", annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(
            rule.stream_plugin_store_key,
            Some("sandbox/sandbox-stream-plugins".to_string())
        );
    }

    #[test]
    fn test_stream_plugin_store_key_short_name() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/edgion-stream-plugins".to_string(), "my-plugins".to_string());

        let route = create_annotated_tls_route("sandbox", "sp-route", "gw1", "test.com", annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.stream_plugin_store_key, Some("sandbox/my-plugins".to_string()));
    }

    #[test]
    fn test_combined_annotations() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/proxy-protocol".to_string(), "v2".to_string());
        annotations.insert("edgion.io/upstream-tls".to_string(), "TRUE".to_string());
        annotations.insert("edgion.io/edgion-stream-plugins".to_string(), "ns/plugins".to_string());

        let route = create_annotated_tls_route("default", "combo", "gw1", "*.sandbox.com", annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, Some(2));
        assert!(rule.upstream_tls);
        assert_eq!(rule.stream_plugin_store_key, Some("ns/plugins".to_string()));
    }

    #[test]
    fn test_no_annotations() {
        let managers = GlobalTlsRouteManagers::new();
        let route = create_test_tls_route("default", "plain", "gw1", "test.com");
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, None);
        assert!(!rule.upstream_tls);
        assert!(rule.stream_plugin_store_key.is_none());
    }
}

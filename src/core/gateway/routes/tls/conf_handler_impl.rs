use super::{cap_tls_connect_retries, DEFAULT_TLS_CONNECT_RETRIES, MAX_TLS_CONNECT_RETRIES};
use crate::core::common::conf_sync::traits::{arc_map_into_owned, ConfHandler};
use crate::core::gateway::routes::common::is_core_service_backend_ref;
use crate::core::gateway::routes::l4_partial_update::apply_l4_partial_update;
use crate::core::gateway::routes::tls::routes_mgr::resolved_listeners_for_route;
use crate::core::gateway::routes::tls::{get_global_tls_route_managers, GlobalTlsRouteManagers};
use crate::types::{ResourceMeta, TLSRoute};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Create a TLS route handler for registration with ConfigClient.
pub fn create_tls_route_handler() -> Box<dyn ConfHandler<TLSRoute> + Send + Sync> {
    Box::new(get_global_tls_route_managers())
}

impl ConfHandler<TLSRoute> for GlobalTlsRouteManagers {
    fn full_set(&self, data: &HashMap<String, Arc<TLSRoute>>) {
        let start_time = Instant::now();
        tracing::info!(
            component = "global_tls_route_managers",
            cnt = data.len(),
            "full set start"
        );

        self.clear_route_cache();

        for (key, route) in data {
            match self.initialize_route((**route).clone()) {
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

        self.rebuild_all_listener_managers();
        tracing::info!(
            component = "global_tls_route_managers",
            ms = Instant::now().saturating_duration_since(start_time).as_millis(),
            "full set done"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<TLSRoute>>,
        update: HashMap<String, Arc<TLSRoute>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "global_tls_route_managers",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let add_owned = arc_map_into_owned(add);
        let update_owned = arc_map_into_owned(update);

        apply_l4_partial_update(
            "TLSRoute",
            add_owned,
            update_owned,
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

use crate::types::constants::annotations::edgion::EDGION_STREAM_PLUGINS;

impl GlobalTlsRouteManagers {
    /// Initialize a TLSRoute by setting up BackendSelector, proxy protocol,
    /// and stream plugin store key from annotations.
    fn initialize_route(&self, mut route: TLSRoute) -> Result<Arc<TLSRoute>, String> {
        let route_key = route.key_name();
        let annotations = route.metadata.annotations.as_ref();

        let proxy_protocol_version = annotations
            .and_then(|a| a.get(crate::types::constants::annotations::edgion::PROXY_PROTOCOL))
            .and_then(|v| match v.trim() {
                "v2" => Some(2u8),
                _ => None,
            });

        let store_key = Self::resolve_stream_plugin_store_key(&route);

        let configured_max_connect_retries = annotations
            .and_then(|a| a.get(crate::types::constants::annotations::edgion::MAX_CONNECT_RETRIES))
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|v| v.max(1))
            .unwrap_or(DEFAULT_TLS_CONNECT_RETRIES);
        let max_connect_retries = cap_tls_connect_retries(configured_max_connect_retries);

        if configured_max_connect_retries > MAX_TLS_CONNECT_RETRIES {
            tracing::warn!(
                route = %route_key,
                configured_max_connect_retries,
                max_connect_retries,
                "TLSRoute max_connect_retries capped"
            );
        }

        if let Some(rules) = route.spec.rules.as_mut() {
            for (rule_idx, rule) in rules.iter_mut().enumerate() {
                if let Some(backend_refs) = &rule.backend_refs {
                    let (backends, weights): (Vec<_>, Vec<_>) = backend_refs
                        .iter()
                        .filter(|br| is_core_service_backend_ref(br.group.as_deref(), br.kind.as_deref()))
                        .map(|br| (br.clone(), br.weight))
                        .unzip();

                    rule.backend_finder.init(backends, weights);

                    tracing::debug!(
                        route = %route_key,
                        rule_idx,
                        backend_count = backend_refs.len(),
                        "Initialized BackendSelector for TLSRoute rule"
                    );
                }

                rule.proxy_protocol_version = proxy_protocol_version;
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

        if proxy_protocol_version.is_some() || max_connect_retries > 1 {
            tracing::info!(
                route = %route_key,
                proxy_protocol = ?proxy_protocol_version,
                max_connect_retries,
                "TLSRoute configured with extended options"
            );
        }

        Ok(Arc::new(route))
    }

    fn resolve_stream_plugin_store_key(route: &TLSRoute) -> Option<String> {
        let annotations = route.metadata.annotations.as_ref()?;
        let annotation_value = annotations.get(EDGION_STREAM_PLUGINS)?;
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
    use crate::types::ListenerKey;

    fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
        ListenerKey::new(addr, protocol)
    }

    fn create_test_tls_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        hostname: &str,
        port: u16,
        listener_key: ListenerKey,
    ) -> TLSRoute {
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
                    port: Some(port as i32),
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

                    stream_plugin_store_key: None,
                    max_connect_retries: 1,
                }]),
                resolved_listeners: Some(vec![listener_key]),
                effective_hostnames: Some(vec![hostname.to_string()]),
            },
            status: None,
        }
    }

    fn create_annotated_tls_route(
        namespace: &str,
        name: &str,
        gateway: &str,
        hostname: &str,
        port: u16,
        listener_key: ListenerKey,
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
                    port: Some(port as i32),
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

                    stream_plugin_store_key: None,
                    max_connect_retries: 1,
                }]),
                resolved_listeners: Some(vec![listener_key]),
                effective_hostnames: Some(vec![hostname.to_string()]),
            },
            status: None,
        }
    }

    #[test]
    fn test_tls_route_full_set() {
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        let mut data = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com", 443, key443.clone());
        let route2 = create_test_tls_route("default", "route2", "gateway1", "api.example.com", 443, key443.clone());

        data.insert("default/route1".to_string(), Arc::new(route1));
        data.insert("default/route2".to_string(), Arc::new(route2));

        managers.full_set(&data);

        let mgr = managers.get_or_create_listener_manager(&key443);
        let table = mgr.load_route_table();
        assert!(table.match_route("test.example.com").is_some());
        assert!(table.match_route("api.example.com").is_some());
        assert!(table.match_route("other.example.com").is_none());
    }

    #[test]
    fn test_tls_route_partial_update() {
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        let mut add = HashMap::new();
        let route1 = create_test_tls_route("default", "route1", "gateway1", "test.example.com", 443, key443.clone());
        add.insert("default/route1".to_string(), Arc::new(route1));

        managers.partial_update(add, HashMap::new(), HashSet::new());

        let mgr = managers.get_or_create_listener_manager(&key443);
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
        let key = make_listener_key(443, "TLS");

        let route = create_annotated_tls_route("default", "pp2-route", "gw1", "*.sandbox.com", 443, key, annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, Some(2));
        assert!(rule.stream_plugin_store_key.is_none());
    }

    #[test]
    fn test_proxy_protocol_annotation_invalid() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/proxy-protocol".to_string(), "v1".to_string());
        let key = make_listener_key(443, "TLS");

        let route = create_annotated_tls_route("default", "pp1-route", "gw1", "*.sandbox.com", 443, key, annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, None);
    }

    #[test]
    fn test_stream_plugin_store_key_full_path() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert(
            "edgion.io/edgion-stream-plugins".to_string(),
            "sandbox/sandbox-stream-plugins".to_string(),
        );
        let key = make_listener_key(443, "TLS");

        let route = create_annotated_tls_route("default", "sp-route", "gw1", "test.com", 443, key, annotations);
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
        let key = make_listener_key(443, "TLS");

        let route = create_annotated_tls_route("sandbox", "sp-route", "gw1", "test.com", 443, key, annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.stream_plugin_store_key, Some("sandbox/my-plugins".to_string()));
    }

    #[test]
    fn test_combined_annotations() {
        let managers = GlobalTlsRouteManagers::new();
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("edgion.io/proxy-protocol".to_string(), "v2".to_string());
        annotations.insert("edgion.io/edgion-stream-plugins".to_string(), "ns/plugins".to_string());
        let key = make_listener_key(443, "TLS");

        let route = create_annotated_tls_route("default", "combo", "gw1", "*.sandbox.com", 443, key, annotations);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, Some(2));
        assert_eq!(rule.stream_plugin_store_key, Some("ns/plugins".to_string()));
    }

    #[test]
    fn test_no_annotations() {
        let managers = GlobalTlsRouteManagers::new();
        let key = make_listener_key(443, "TLS");
        let route = create_test_tls_route("default", "plain", "gw1", "test.com", 443, key);
        let initialized = managers.initialize_route(route).unwrap();

        let rule = initialized.spec.rules.as_ref().unwrap().first().unwrap();
        assert_eq!(rule.proxy_protocol_version, None);
        assert!(rule.stream_plugin_store_key.is_none());
        assert_eq!(rule.max_connect_retries, 2);
    }

    #[test]
    fn test_misclassified_add_rebuilds_old_listener() {
        // Simulates upstream misclassification: an existing key is sent in
        // the `add` bucket. Bug 2: pre-collection of affected listeners
        // excludes keys in the `add` bucket, so L443 is not scheduled for
        // rebuild and retains stale route data.

        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");
        let key8443 = make_listener_key(8443, "TLS");

        // Setup: route-x bound to L443
        let mut add = HashMap::new();
        add.insert(
            "default/route-x".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-x",
                "gw1",
                "v1.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());
        let mgr443 = managers.get_or_create_listener_manager(&key443);
        assert!(
            mgr443.load_route_table().match_route("v1.example.com").is_some(),
            "test setup failed: v1.example.com should be in L443"
        );

        // Misclassified add: same key, new version bound to L8443
        // (L8443 doesn't exist yet — it will be created lazily)
        let mut misclassified_add = HashMap::new();
        misclassified_add.insert(
            "default/route-x".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-x",
                "gw1",
                "v2.example.com",
                8443,
                key8443.clone(),
            )),
        );
        managers.partial_update(misclassified_add, HashMap::new(), HashSet::new());

        // L443 must be rebuilt: v1.example.com should be gone
        assert!(
            mgr443.load_route_table().match_route("v1.example.com").is_none(),
            "Bug 2: L443 still has stale route after misclassified add"
        );
        // L8443 should have the new route (created lazily during partial_update)
        let mgr8443 = managers.get_or_create_listener_manager(&key8443);
        assert!(
            mgr8443.load_route_table().match_route("v2.example.com").is_some(),
            "Bug 2 (corollary): L8443 should have the new route"
        );
    }

    #[test]
    fn test_same_batch_add_remove_same_listener() {
        // Same batch: remove route-a and add route-b on the same listener.
        // Verifies the final state has only route-b (no stale route-a, no
        // missing route-b).
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        let mut add = HashMap::new();
        add.insert(
            "default/route-a".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-a",
                "gw1",
                "old.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());

        // Single batch: remove route-a, add route-b on same listener
        let mut add2 = HashMap::new();
        add2.insert(
            "default/route-b".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-b",
                "gw1",
                "new.example.com",
                443,
                key443.clone(),
            )),
        );
        let mut remove = HashSet::new();
        remove.insert("default/route-a".to_string());
        managers.partial_update(add2, HashMap::new(), remove);

        let mgr = managers.get_or_create_listener_manager(&key443);
        let table = mgr.load_route_table();
        assert!(
            table.match_route("new.example.com").is_some(),
            "route-b should be routable"
        );
        assert!(
            table.match_route("old.example.com").is_none(),
            "route-a should be removed"
        );
    }

    #[test]
    fn test_update_listener_migration() {
        // Update moves a route from one listener to another.
        // Both the source (L443) and destination (L9443) listener managers
        // must be rebuilt: L443 loses the route, L9443 gains it.
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");
        let key9443 = make_listener_key(9443, "TLS");

        let mut add = HashMap::new();
        add.insert(
            "default/route-y".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-y",
                "gw1",
                "svc.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(add, HashMap::new(), HashSet::new());

        // Migrate route-y from L443 to L9443
        let mut update = HashMap::new();
        update.insert(
            "default/route-y".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-y",
                "gw1",
                "svc.example.com",
                9443,
                key9443.clone(),
            )),
        );
        managers.partial_update(HashMap::new(), update, HashSet::new());

        let mgr443 = managers.get_or_create_listener_manager(&key443);
        assert!(
            mgr443.load_route_table().match_route("svc.example.com").is_none(),
            "L443 should not have route after migration"
        );
        let mgr9443 = managers.get_or_create_listener_manager(&key9443);
        assert!(
            mgr9443.load_route_table().match_route("svc.example.com").is_some(),
            "L9443 should have route after migration"
        );
    }

    #[test]
    fn test_update_nonexistent_key_is_safe() {
        // Updating a key that doesn't exist in cache (remove_cached is a no-op,
        // init_and_insert runs, new route is inserted). Verifies no panic and
        // the new route appears in the target listener.
        let managers = GlobalTlsRouteManagers::new();
        let key443 = make_listener_key(443, "TLS");

        // Update a key that was never added — behaves like an add
        let mut update = HashMap::new();
        update.insert(
            "default/route-ghost".to_string(),
            Arc::new(create_test_tls_route(
                "default",
                "route-ghost",
                "gw1",
                "ghost.example.com",
                443,
                key443.clone(),
            )),
        );
        managers.partial_update(HashMap::new(), update, HashSet::new());

        let mgr = managers.get_or_create_listener_manager(&key443);
        assert!(
            mgr.load_route_table().match_route("ghost.example.com").is_some(),
            "update of nonexistent key should insert the new route"
        );
    }
}

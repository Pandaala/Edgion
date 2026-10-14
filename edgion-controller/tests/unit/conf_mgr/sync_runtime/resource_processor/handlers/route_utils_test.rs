use edgion_resources::resources::http_route::LocalObjectReference;

#[tokio::test]
async fn ordinary_http_and_grpc_parent_acceptance_and_resolution_ignore_listener_admission_status() {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use serde_json::json;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor.clone());
    let mut gateway: Gateway = serde_json::from_value(json!({
        "metadata": {"name": "gateway", "namespace": "apps"},
        "spec": {"gatewayClassName": "test", "listeners": [
            {"name": "shared", "protocol": "HTTP", "port": 8080},
            {"name": "healthy", "protocol": "HTTP", "port": 8081}
        ]}
    }))
    .unwrap();
    let parent: ParentReference = serde_json::from_value(json!({"name": "gateway"})).unwrap();
    // The baseline filesystem fixture has no Kubernetes ownership identity.
    // Neither absent publication status nor partial/full shared rejection adds
    // an ordinary Route acceptance or attachment-resolution precondition.
    for rejected_listeners in [0, 1, 2] {
        gateway.spec.current_status = if rejected_listeners == 0 {
            None
        } else {
            Some(serde_json::from_value(json!({
                "listeners": (["shared", "healthy"].into_iter().enumerate().map(|(index, name)| json!({
                    "name": name, "supportedKinds": [], "attachedRoutes": 0,
                    "conditions": if index < rejected_listeners {
                        json!([
                            {"type": "Accepted", "status": "False", "reason": "SharedListenerPolicyConflict", "message": "Blocked by apps/owner", "lastTransitionTime": "2026-10-14T00:00:00Z"},
                            {"type": "Programmed", "status": "False", "reason": "Pending", "message": "Listener rejected", "lastTransitionTime": "2026-10-14T00:00:00Z"}
                        ])
                    } else { json!([]) }
                })).collect::<Vec<_>>())
            })).unwrap())
        };
        raw.upsert("apps/gateway".into(), Arc::new(gateway.clone()));
        if rejected_listeners != 0 {
            processor.save(gateway.clone(), None);
        }
        for kind in ["HTTPRoute", "GRPCRoute"] {
            let accepted = validate_parent_ref_accepted("apps", &parent, None, kind, &runtime);
            assert!(accepted.errors.is_empty(), "listener status must not reject {kind}");
            assert_eq!(accepted.matched_listeners, ["shared", "healthy"]);
        }
        let keys = resolve_listener_keys(std::slice::from_ref(&parent), "apps", Some("apps"), &runtime);
        assert_eq!(
            keys.len(),
            2,
            "listener status must not remove ordinary Route attachments"
        );
    }
}

#[test]
fn invalid_patterns_never_intersect() {
    for invalid in ["*", "A.example.com", "127.0.0.1", "a..b"] {
        assert!(!super::hostnames_intersect(invalid, invalid));
    }
    assert!(super::hostnames_intersect("a", "a"));
    assert!(super::hostnames_intersect("*.127.0.0.1", "a.127.0.0.1"));
}

#[tokio::test]
async fn tls_hostnames_remain_scoped_to_attachments_through_updates() {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let gateway = |name: &str, port: u16, hostname: Option<&str>| {
        Arc::new(
            serde_json::from_value::<Gateway>(serde_json::json!({
                "metadata": {"namespace": "apps", "name": name},
                "spec": {"gatewayClassName": "test", "listeners": [{
                    "name": "tls", "port": port, "protocol": "TLS", "hostname": hostname,
                    "tls": {"mode": "Passthrough"}
                }]}
            }))
            .unwrap(),
        )
    };
    raw.upsert("apps/a".into(), gateway("a", 8443, Some("a.example.com")));
    raw.upsert("apps/b".into(), gateway("b", 9443, Some("b.example.com")));
    let parents: Vec<ParentReference> = serde_json::from_value(serde_json::json!([
        {"name": "a", "sectionName": "tls"}, {"name": "b", "sectionName": "tls"}
    ]))
    .unwrap();
    let hosts = vec!["*.example.com".to_string()];
    let resolved = resolve_tls_listener_hostnames(&parents, "apps", &hosts, &runtime);
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].hostnames, ["a.example.com"]);
    assert_eq!(resolved[1].hostnames, ["b.example.com"]);
    assert_eq!(resolved[0].gateway_name, "a");
    assert_eq!(resolved[1].listener_key.0, "0.0.0.0:9443/TLS");
    // An unrestricted listener preserves the explicit route wildcard; it
    // does not widen the other attachment or manufacture a route catch-all.
    raw.upsert("apps/a".into(), gateway("a", 8443, None));
    let updated = resolve_tls_listener_hostnames(&parents, "apps", &hosts, &runtime);
    assert_eq!(updated[0].hostnames, ["*.example.com"]);
    assert_eq!(updated[1], resolved[1]);
    raw.upsert("apps/a".into(), gateway("a", 10443, Some("unrelated.test")));
    let narrowed = resolve_tls_listener_hostnames(&parents, "apps", &hosts, &runtime);
    assert_eq!(narrowed, [resolved[1].clone()]);
    assert!(resolve_tls_listener_hostnames(&parents, "apps", &[], &runtime).is_empty());
    let port_mismatch: Vec<ParentReference> = serde_json::from_value(serde_json::json!([
        {"name": "b", "sectionName": "tls", "port": 8443}
    ]))
    .unwrap();
    assert!(resolve_tls_listener_hostnames(&port_mismatch, "apps", &hosts, &runtime).is_empty());
    let mut mixed = (*gateway("a", 8443, Some("a.example.com"))).clone();
    mixed.spec.listeners.as_mut().unwrap().push(
        serde_json::from_value(serde_json::json!({
            "name": "wrong-group", "port": 8443, "protocol": "TLS", "hostname": "a.example.com",
            "allowedRoutes": {"kinds": [{"kind": "TLSRoute", "group": "unrelated.example.com"}]}
        }))
        .unwrap(),
    );
    raw.upsert("apps/a".into(), Arc::new(mixed));
    let all: Vec<ParentReference> = serde_json::from_value(serde_json::json!([{"name": "a"}])).unwrap();
    let filtered = resolve_tls_listener_hostnames(&all, "apps", &hosts, &runtime);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].listener_name, "tls");
}

#[tokio::test]
async fn explicit_route_kind_cannot_override_listener_protocol() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);

    let gateway = |tls_protocol: &str| {
        Arc::new(
            serde_json::from_value::<Gateway>(serde_json::json!({
                "metadata": {"namespace": "apps", "name": "gateway"},
                "spec": {"gatewayClassName": "test", "listeners": [
                    {
                        "name": "incompatible", "port": 8080, "protocol": "HTTP",
                        "allowedRoutes": {"kinds": [{"kind": "TLSRoute"}]}
                    },
                    {
                        "name": "compatible", "port": 8443, "protocol": tls_protocol,
                        "allowedRoutes": {"kinds": [{"kind": "TLSRoute"}]}
                    }
                ]}
            }))
            .unwrap(),
        )
    };
    let parent: ParentReference = serde_json::from_value(serde_json::json!({"name": "gateway"})).unwrap();

    raw.upsert("apps/gateway".into(), gateway("TLS"));
    let accepted = validate_parent_ref_accepted("apps", &parent, None, "TLSRoute", &runtime);
    assert!(accepted.errors.is_empty());
    assert_eq!(accepted.matched_listeners, ["compatible"]);
    assert!(update_tracker_accepted_only(
        ResourceKind::TLSRoute,
        "apps",
        "route",
        Some(&vec![parent.clone()]),
        None,
        &runtime,
    ));
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "incompatible"),
        0
    );
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "compatible"),
        1
    );

    raw.upsert("apps/gateway".into(), gateway("HTTP"));
    let rejected = validate_parent_ref_accepted("apps", &parent, None, "TLSRoute", &runtime);
    assert_eq!(rejected.errors.len(), 1);
    assert_eq!(rejected.errors[0].reason(), "NotAllowedByListeners");
    assert!(update_tracker_accepted_only(
        ResourceKind::TLSRoute,
        "apps",
        "route",
        Some(&vec![parent.clone()]),
        None,
        &runtime,
    ));
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "incompatible"),
        0
    );
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "compatible"),
        0
    );

    raw.upsert("apps/gateway".into(), gateway("TLS"));
    assert!(update_tracker_accepted_only(
        ResourceKind::TLSRoute,
        "apps",
        "route",
        Some(&vec![parent]),
        None,
        &runtime,
    ));
    assert_eq!(
        runtime
            .attached_route_tracker
            .count_for_listener("apps/gateway", "compatible"),
        1
    );
}

#[tokio::test]
async fn backend_request_id_rejects_only_active_entries() {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let reference: LocalObjectReference = serde_json::from_value(serde_json::json!({
        "group": "edgion.io", "kind": "EdgionPlugins", "name": "ids"
    }))
    .unwrap();
    for enabled in [true, false, true] {
        let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
            "metadata": {"namespace": "apps", "name": "ids"},
            "spec": {"requestPlugins": [{"type": "RequestId", "enable": enabled, "config": {}}]}
        }))
        .unwrap();
        raw.upsert("apps/ids".to_string(), Arc::new(plugins));
        let owner = backend_extension_owner(&reference, 2);
        let root = ResourceRef::new(ResourceKind::HTTPRoute, Some("apps".into()), "route".into());
        let field = edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs::from_route(&owner)
            .unwrap()
            .authored_at(0, Some(2), 0)
            .unwrap();
        let error = backend_request_scope_extension_ref_error(&runtime, &root, field);
        assert_eq!(error.is_some(), enabled);
        if let Some(error) = error {
            assert!(error.contains("backendRefs[2]"));
            assert!(error.contains("cannot contain RequestId"));
        }
    }
}

#[tokio::test]
async fn backend_scope_rejects_active_upstream_jump_plugins() {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let reference: LocalObjectReference = serde_json::from_value(serde_json::json!({
        "group": "edgion.io", "kind": "EdgionPlugins", "name": "jump"
    }))
    .unwrap();
    use edgion_resources::resources::edgion_plugins::{
        DirectEndpointConfig, DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig, RegionRouteConfig,
    };
    for (plugin_type, config) in [
        (
            "DirectEndpoint",
            serde_json::to_value(DirectEndpointConfig::default()).unwrap(),
        ),
        (
            "DynamicInternalUpstream",
            serde_json::to_value(DynamicInternalUpstreamConfig::default()).unwrap(),
        ),
        (
            "DynamicExternalUpstream",
            serde_json::to_value(DynamicExternalUpstreamConfig::default()).unwrap(),
        ),
        (
            "RegionRoute",
            serde_json::to_value(RegionRouteConfig::default()).unwrap(),
        ),
        (
            "Canary",
            serde_json::json!({"enable": true, "activeProfile": "main", "profiles": {"main": {"name": "svc"}}}),
        ),
    ] {
        for enabled in [true, false] {
            let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
                "metadata": {"namespace": "apps", "name": "jump"},
                "spec": {"requestPlugins": [{"type": plugin_type, "enable": enabled, "config": config}]}
            }))
            .unwrap();
            raw.upsert("apps/jump".to_string(), Arc::new(plugins));
            let owner = backend_extension_owner(&reference, 0);
            let root = ResourceRef::new(ResourceKind::HTTPRoute, Some("apps".into()), "route".into());
            let field = edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs::from_route(&owner)
                .unwrap()
                .authored_at(0, Some(0), 0)
                .unwrap();
            let error = backend_request_scope_extension_ref_error(&runtime, &root, field);
            assert_eq!(error.is_some(), enabled, "{plugin_type}");
            if let Some(error) = error {
                assert!(error.contains(plugin_type), "{error}");
            }
        }
    }
}

use super::*;
use edgion_resources::resources::gateway::{AllowedRoutes, RouteNamespaces};

#[test]
fn test_hostnames_intersect_concrete() {
    assert!(hostnames_intersect("foo.com", "foo.com"));
    assert!(!hostnames_intersect("foo.com", "bar.com"));
}

#[test]
fn test_hostnames_intersect_wildcard_listener_concrete_route() {
    assert!(hostnames_intersect("*.wildcard.io", "foo.wildcard.io"));
    assert!(hostnames_intersect("*.bar.com", "a.b.bar.com"));
    assert!(!hostnames_intersect("*.wildcard.io", "wildcard.io"));
    assert!(!hostnames_intersect("*.bar.com", "foo.com"));
}

#[test]
fn test_hostnames_intersect_concrete_listener_wildcard_route() {
    assert!(hostnames_intersect("very.specific.com", "*.specific.com"));
    assert!(!hostnames_intersect("specific.com", "*.specific.com"));
}

#[test]
fn test_hostnames_intersect_both_wildcards() {
    assert!(hostnames_intersect("*.bar.com", "*.foo.bar.com"));
    assert!(hostnames_intersect("*.foo.bar.com", "*.bar.com"));
    assert!(hostnames_intersect("*.bar.com", "*.bar.com"));
    assert!(!hostnames_intersect("*.bar.com", "*.foo.com"));
}

#[test]
fn test_ns_policy_none_defaults_to_same() {
    let runtime = crate::conf_mgr::ControllerRuntime::default();
    assert!(listener_allows_route_namespace(&None, "ns1", "ns1", &runtime));
    assert!(!listener_allows_route_namespace(&None, "ns1", "ns2", &runtime));
}

#[test]
fn test_resolve_listener_keys_skips_delegation_parent_refs() {
    // A `kind: HTTPRoute` parentRef (delegation reverse lock) must not be
    // looked up as a Gateway — it yields no listener keys and no side
    // effects (previously it would log a bogus "Gateway not found").
    let runtime = crate::conf_mgr::ControllerRuntime::default();
    let delegation_ref = ParentReference {
        group: Some("gateway.networking.k8s.io".to_string()),
        kind: Some("HTTPRoute".to_string()),
        namespace: Some("infra".to_string()),
        name: "parent-route".to_string(),
        section_name: None,
        port: None,
    };
    let keys = resolve_listener_keys(&[delegation_ref], "billing", Some("billing"), &runtime);
    assert!(keys.is_empty());
}

#[test]
fn test_ns_policy_all() {
    let runtime = crate::conf_mgr::ControllerRuntime::default();
    let allowed = Some(AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("All".into()),
            selector: None,
        }),
        kinds: None,
    });
    assert!(listener_allows_route_namespace(&allowed, "any-ns", "gw-ns", &runtime));
}

#[test]
fn test_ns_policy_same() {
    let runtime = crate::conf_mgr::ControllerRuntime::default();
    let allowed = Some(AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("Same".into()),
            selector: None,
        }),
        kinds: None,
    });
    assert!(listener_allows_route_namespace(&allowed, "ns1", "ns1", &runtime));
    assert!(!listener_allows_route_namespace(&allowed, "ns1", "ns2", &runtime));
}

#[test]
fn test_ns_policy_selector_no_selector_field() {
    let runtime = crate::conf_mgr::ControllerRuntime::default();
    let allowed = Some(AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("Selector".into()),
            selector: None,
        }),
        kinds: None,
    });
    assert!(listener_allows_route_namespace(&allowed, "any-ns", "gw-ns", &runtime));
}

#[test]
fn test_ns_policy_selector_falls_back_to_same_when_labels_missing() {
    let runtime = crate::conf_mgr::ControllerRuntime::default();
    let allowed = Some(AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("Selector".into()),
            selector: Some(serde_json::json!({ "matchLabels": { "env": "prod" } })),
        }),
        kinds: None,
    });
    assert!(listener_allows_route_namespace(&allowed, "gw-ns", "gw-ns", &runtime));
    assert!(!listener_allows_route_namespace(
        &allowed, "other-ns", "gw-ns", &runtime
    ));
}

#[test]
fn test_ns_policy_selector_with_store() {
    use std::collections::BTreeMap;

    let runtime = crate::conf_mgr::ControllerRuntime::default();
    runtime.namespace_store.upsert(k8s_openapi::api::core::v1::Namespace {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some("allowed-ns".into()),
            labels: Some(BTreeMap::from([("env".into(), "prod".into())])),
            ..Default::default()
        },
        ..Default::default()
    });
    runtime.namespace_store.upsert(k8s_openapi::api::core::v1::Namespace {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some("denied-ns".into()),
            labels: Some(BTreeMap::from([("env".into(), "dev".into())])),
            ..Default::default()
        },
        ..Default::default()
    });

    let allowed = Some(AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("Selector".into()),
            selector: Some(serde_json::json!({ "matchLabels": { "env": "prod" } })),
        }),
        kinds: None,
    });

    assert!(listener_allows_route_namespace(
        &allowed,
        "allowed-ns",
        "gw-ns",
        &runtime
    ));
    assert!(!listener_allows_route_namespace(
        &allowed,
        "denied-ns",
        "gw-ns",
        &runtime
    ));
    assert!(!listener_allows_route_namespace(
        &allowed,
        "unknown-ns",
        "gw-ns",
        &runtime
    ));

    runtime.namespace_store.remove("allowed-ns");
    runtime.namespace_store.remove("denied-ns");
}

#[tokio::test]
async fn cross_namespace_parent_rejects_invalid_not_in_values() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use serde_json::json;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(GatewayHandler::new(None, None)),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let set_namespace = |team: &str| {
        runtime.namespace_store.upsert(
            serde_json::from_value(json!({
                "metadata": {"name": "apps", "labels": {"team": team}}
            }))
            .unwrap(),
        );
    };
    set_namespace("restricted");
    let parent: ParentReference = serde_json::from_value(json!({"name": "gateway", "namespace": "infra"})).unwrap();
    for (expression, expected) in [
        (json!({"key": "team", "operator": "NotIn"}), false),
        (json!({"key": "team", "operator": "NotIn", "values": []}), false),
        (json!({"key": "team", "operator": "NotIn", "values": ["other"]}), true),
    ] {
        let gateway: Gateway = serde_json::from_value(json!({
            "metadata": {"namespace": "infra", "name": "gateway"},
            "spec": {"gatewayClassName": "test", "listeners": [{
                "name": "http", "port": 80, "protocol": "HTTP",
                "allowedRoutes": {"namespaces": {"from": "Selector", "selector": {"matchExpressions": [expression]}}}
            }]}
        }))
        .unwrap();
        raw.upsert("infra/gateway".into(), Arc::new(gateway));
        let result = validate_parent_ref_accepted("apps", &parent, None, "HTTPRoute", &runtime);
        if expected {
            assert!(result.errors.is_empty());
            assert_eq!(result.matched_listeners, ["http"]);
        } else {
            assert_eq!(result.errors.len(), 1);
            assert_eq!(result.errors[0].reason(), "NotAllowedByListeners");
            assert!(result.matched_listeners.is_empty());
        }
    }
    // Re-evaluation uses the refreshed committed labels with the valid selector.
    set_namespace("other");
    let result = validate_parent_ref_accepted("apps", &parent, None, "HTTPRoute", &runtime);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].reason(), "NotAllowedByListeners");
    assert!(result.matched_listeners.is_empty());
}

fn backend_extension_owner(reference: &LocalObjectReference, index: usize) -> edgion_resources::HTTPRoute {
    let mut backends = vec![serde_json::json!({"name":"service","port":80}); index + 1];
    backends[index]["filters"] = serde_json::json!([{"type":"ExtensionRef","extensionRef":reference}]);
    serde_json::from_value(serde_json::json!({"metadata":{"name":"route","namespace":"apps","uid":"declaring-uid","generation":9},"spec":{"rules":[{"backendRefs":backends}]}})).unwrap()
}

#[tokio::test]
async fn extension_lookup_registers_before_desired_generation_read_and_rejects_invalid_fields() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs;
    use std::cell::Cell;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor.clone());
    let reference: LocalObjectReference = serde_json::from_value(serde_json::json!({
        "group":"edgion.io","kind":"EdgionPlugins","name":"bundle","resolvedNamespace":"forged"
    }))
    .unwrap();
    let route = backend_extension_owner(&reference, 2);
    let field = HttpRouteExtensionRefs::from_route(&route)
        .unwrap()
        .authored_at(0, Some(2), 0)
        .unwrap();
    let root = ResourceRef::new(ResourceKind::HTTPRoute, None, "wake-root".into());
    let reads = Cell::new(0);
    let inspect = |field| {
        extension_scope_violation_with(
            field,
            true,
            |receipt| {
                assert_eq!(receipt._source.namespace.as_deref(), Some("apps"));
                assert_eq!(receipt._source_uid.as_deref(), Some("declaring-uid"));
                assert_eq!(receipt._source_generation, Some(9));
                assert_eq!(receipt._slot.backend(), Some(2));
                receipt.register(&root, &runtime);
            },
            |key| {
                reads.set(reads.get() + 1);
                assert_eq!(key, "apps/bundle", "authored stamp never changes target");
                assert_eq!(
                    runtime.plugin_ref_mgr.get_dependencies(&root.key()),
                    vec![key.to_string()]
                );
                runtime
                    .processor_registry
                    .get("EdgionPlugins")?
                    .get_raw_any(key)?
                    .downcast::<EdgionPlugins>()
                    .ok()
            },
        )
    };
    assert_eq!(
        inspect(field),
        None,
        "a valid missing target still registers before querying"
    );
    let make_plugins = |enabled, generation| {
        serde_json::from_value::<EdgionPlugins>(serde_json::json!({
            "metadata":{"namespace":"apps","name":"bundle","generation":generation},
            "spec":{"requestPlugins":[{"type":"RequestId","enable":enabled,"config":{}}]}
        }))
        .unwrap()
    };
    processor.save(make_plugins(true, 1), None);
    raw.upsert("apps/bundle".into(), Arc::new(make_plugins(false, 2)));
    assert_eq!(
        inspect(field),
        None,
        "desired raw generation overrides an active stale processed cache"
    );
    raw.upsert("apps/bundle".into(), Arc::new(make_plugins(true, 3)));
    assert_eq!(inspect(field), Some(("RequestId", true)));
    raw.remove("apps/bundle");
    assert_eq!(
        inspect(field),
        None,
        "deletion must not fall back to processed material"
    );
    raw.upsert("apps/bundle".into(), Arc::new(make_plugins(true, 1)));
    assert_eq!(
        inspect(field),
        Some(("RequestId", true)),
        "recreated desired material is observed"
    );
    assert_eq!(reads.get(), 5);

    for (key, value) in [
        ("group", ""),
        ("group", "core"),
        ("kind", "Secret"),
        ("name", ""),
        ("name", "bad/name"),
    ] {
        let mut bad = serde_json::to_value(&route).unwrap();
        bad["spec"]["rules"][0]["backendRefs"][2]["filters"][0]["extensionRef"][key] = serde_json::json!(value);
        let bad = serde_json::from_value(bad).unwrap();
        let field = HttpRouteExtensionRefs::from_route(&bad)
            .unwrap()
            .authored_at(0, Some(2), 0)
            .unwrap();
        assert_eq!(
            extension_scope_violation_with(
                field,
                true,
                |_| panic!("invalid target registered"),
                |_| panic!("invalid target read")
            ),
            None
        );
    }
    let mut effective = route.clone();
    effective.spec.resolved_rules = effective.spec.rules.clone();
    effective.spec.resolved_rules.as_mut().unwrap()[0]
        .backend_refs
        .as_mut()
        .unwrap()[2]
        .filters
        .as_mut()
        .unwrap()[0]
        .extension_ref
        .as_mut()
        .unwrap()
        .resolved_namespace = Some("bad/ns".into());
    let field = HttpRouteExtensionRefs::from_route(&effective)
        .unwrap()
        .effective_at(0, Some(2), 0)
        .unwrap();
    assert_eq!(
        extension_scope_violation_with(
            field,
            true,
            |_| panic!("invalid stamp registered"),
            |_| panic!("invalid stamp read")
        ),
        None
    );
    for (key, value) in [
        ("name", ""),
        ("name", "bad/name"),
        ("namespace", ""),
        ("namespace", "Bad"),
    ] {
        let mut bad = serde_json::to_value(&route).unwrap();
        bad["metadata"][key] = serde_json::json!(value);
        let bad = serde_json::from_value(bad).unwrap();
        assert!(HttpRouteExtensionRefs::from_route(&bad).is_err());
    }
}

#[test]
fn registration_only_extension_pass_does_not_inspect_ineligible_rule_material() {
    use edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs;
    let reference =
        serde_json::from_value(serde_json::json!({"group":"edgion.io","kind":"EdgionPlugins","name":"bundle"}))
            .unwrap();
    let route = backend_extension_owner(&reference, 0);
    let field = HttpRouteExtensionRefs::from_route(&route)
        .unwrap()
        .authored_at(0, Some(0), 0)
        .unwrap();
    let registered = std::cell::Cell::new(false);
    assert_eq!(
        extension_scope_violation_with(
            field,
            false,
            |_| registered.set(true),
            |_| panic!("structurally ineligible rule queried")
        ),
        None
    );
    assert!(registered.get());
}

#[tokio::test]
async fn raw_http_owner_clear_finalize_preserves_child_mirror_and_policy_ancestry() {
    use crate::conf_mgr::sync_runtime::resource_processor::service_ref::mirror_service_key;
    use crate::conf_mgr::sync_runtime::resource_processor::HandlerContext;
    use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor, ResourceRef};
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
    use edgion_resources::{EdgionBackendTrafficPolicy, ResourceKind};
    use std::sync::Arc;
    struct Noop;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionBackendTrafficPolicy> for Noop {}
    let runtime = Arc::new(ControllerRuntime::default());
    let policies = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
        "EdgionBackendTrafficPolicy",
        16,
        Arc::new(Noop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(policies.clone());
    let root = ResourceRef::new(ResourceKind::HTTPRoute, None, "root".into());
    let policy = ResourceRef::new(
        ResourceKind::EdgionBackendTrafficPolicy,
        Some("default".into()),
        "policy".into(),
    );
    runtime.service_ref_mgr.add_ref("default/forward".into(), root.clone());
    runtime.service_ref_mgr.add_ref("default/forward".into(), policy);
    runtime
        .service_ref_mgr
        .add_ref(mirror_service_key("children", "copy"), root.clone());
    let old = begin_service_refs(&root, &runtime);
    assert_eq!(old, HashSet::from(["default/forward".to_string()]));
    runtime
        .service_ref_mgr
        .add_ref(mirror_service_key("children", "copy"), root.clone());
    let dropped = finish_service_refs(&root, &[], old, &runtime);
    assert_eq!(dropped, vec!["default/forward"]);
    assert_eq!(
        runtime
            .service_ref_mgr
            .get_refs(&mirror_service_key("children", "copy")),
        vec![root.clone()]
    );
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    super::super::requeue_backend_policies_for_services(&dropped, &ctx).await;
    let queue = policies.workqueue();
    let item = queue.dequeue().await.unwrap();
    assert_eq!(item.key, "default/policy");
    queue.done(&item.key);
    runtime.service_ref_mgr.add_ref("default/forward".into(), root.clone());
    super::super::requeue_backend_policies_for_owner(&root, &ctx).await;
    let item = queue.dequeue().await.unwrap();
    assert_eq!(item.key, "default/policy");
    queue.done(&item.key);
    let parent: edgion_resources::resources::common::ParentReference =
        serde_json::from_value(serde_json::json!({"name":"gateway"})).unwrap();
    runtime
        .gateway_route_index
        .update_route(ResourceKind::HTTPRoute, "default/root", &[parent], "default");
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "default".into(),
        name: "forward".into(),
    };
    let ancestors = super::super::resolve_policy_ancestor_gateways_for_targets(&[target], "default/policy", &ctx);
    assert!(ancestors.contains_key("default/gateway"));
    let mirror = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "children".into(),
        name: "copy".into(),
    };
    assert!(super::super::resolve_policy_ancestor_gateways_for_targets(&[mirror], "default/policy", &ctx).is_empty());
}

#[tokio::test]
async fn semantic_plugin_lookup_never_borrows_one_collision_candidate() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs;
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor);
    let mut source: EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"ids"}, "spec":{"requestPlugins":[{"type":"RequestId","config":{}}]}
    }))
    .unwrap();
    raw.upsert("ids".into(), Arc::new(source.clone()));
    let reference: LocalObjectReference =
        serde_json::from_value(serde_json::json!({"group":"edgion.io","kind":"EdgionPlugins","name":"ids"})).unwrap();
    let mut route = backend_extension_owner(&reference, 2);
    route.metadata.namespace = None;
    let refs = HttpRouteExtensionRefs::from_route(&route).unwrap();
    let field = refs.authored_at(0, Some(2), 0).unwrap();
    let root = ResourceRef::new(ResourceKind::HTTPRoute, None, "route".into());
    assert!(backend_request_scope_extension_ref_error(&runtime, &root, field)
        .unwrap()
        .contains("RequestId"));
    source.metadata.namespace = Some("default".into());
    raw.upsert("default/ids".into(), Arc::new(source));
    assert!(matches!(
        plugin_target(&runtime, "default/ids"),
        PluginTarget::Ambiguous
    ));
    assert!(
        backend_request_scope_extension_ref_error(&runtime, &root, field).is_none(),
        "collision must not drop a rule using candidate scope restrictions"
    );
    assert_eq!(
        ambiguous_plugin_ref(field, &runtime).unwrap().reason(),
        "InvalidPluginsRef"
    );
    assert_eq!(
        runtime.plugin_ref_mgr.get_dependencies(&root.key()),
        vec!["default/ids"]
    );
    raw.remove("ids");
    assert!(ambiguous_plugin_ref(field, &runtime).is_none());
    assert!(backend_request_scope_extension_ref_error(&runtime, &root, field)
        .unwrap()
        .contains("RequestId"));
}

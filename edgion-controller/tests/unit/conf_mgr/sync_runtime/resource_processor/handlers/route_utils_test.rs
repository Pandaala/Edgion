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
        let error = backend_request_scope_extension_ref_error(&runtime, "apps", 2, &reference);
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
            let error = backend_request_scope_extension_ref_error(&runtime, "apps", 0, &reference);
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

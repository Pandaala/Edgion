use super::*;

fn pr(ns: &str, name: &str) -> ParentReference {
    ParentReference {
        group: Some("gateway.networking.k8s.io".to_string()),
        kind: Some("Gateway".to_string()),
        namespace: Some(ns.to_string()),
        name: name.to_string(),
        section_name: None,
        port: None,
    }
}

#[test]
fn test_basic_registration() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1")],
        "default",
    );

    let routes = idx.get_routes_for_gateway("default/gw1");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0], (ResourceKind::HTTPRoute, "default/r1".to_string()));
}

#[test]
fn test_multi_gateway() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1"), pr("other", "gw2")],
        "default",
    );

    assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 1);
    assert_eq!(idx.get_routes_for_gateway("other/gw2").len(), 1);
}

#[test]
fn test_update_replaces() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1")],
        "default",
    );
    assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 1);

    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw2")],
        "default",
    );
    assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 0);
    assert_eq!(idx.get_routes_for_gateway("default/gw2").len(), 1);
}

#[test]
fn test_remove() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1")],
        "default",
    );
    assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 1);

    idx.remove_route(ResourceKind::HTTPRoute, "default/r1");
    assert_eq!(idx.get_routes_for_gateway("default/gw1").len(), 0);
}

#[test]
fn test_multiple_routes_same_gateway() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1")],
        "default",
    );
    idx.update_route(
        ResourceKind::GRPCRoute,
        "default/g1",
        &[pr("default", "gw1")],
        "default",
    );

    let routes = idx.get_routes_for_gateway("default/gw1");
    assert_eq!(routes.len(), 2);
}

#[test]
fn test_non_gateway_parent_ref_ignored() {
    let idx = GatewayRouteIndex::new();
    let non_gw_ref = ParentReference {
        group: Some("example.io".to_string()),
        kind: Some("Service".to_string()),
        namespace: Some("default".to_string()),
        name: "svc1".to_string(),
        section_name: None,
        port: None,
    };
    idx.update_route(ResourceKind::HTTPRoute, "default/r1", &[non_gw_ref], "default");

    assert!(idx.get_routes_for_gateway("default/svc1").is_empty());
}

#[test]
fn test_get_gateways_for_route() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1"), pr("other", "gw2")],
        "default",
    );

    let mut gateways = idx.get_gateways_for_route(ResourceKind::HTTPRoute, "default/r1");
    gateways.sort();
    assert_eq!(gateways, vec!["default/gw1", "other/gw2"]);

    // Non-existent route returns empty
    let empty = idx.get_gateways_for_route(ResourceKind::HTTPRoute, "default/nonexistent");
    assert!(empty.is_empty());
}

fn gateway(listeners: serde_json::Value) -> Gateway {
    serde_json::from_value(serde_json::json!({
        "metadata": {"name": "gw1", "namespace": "default"},
        "spec": {"gatewayClassName": "test", "listeners": listeners}
    }))
    .unwrap()
}

fn listener(name: &str, hostname: Option<&str>, port: i32) -> serde_json::Value {
    serde_json::json!({"name": name, "hostname": hostname, "port": port, "protocol": "TLS"})
}

#[test]
fn test_atomic_listener_update_no_change() {
    let idx = GatewayRouteIndex::new();
    let gateway = gateway(serde_json::json!([listener("a", Some("a.example.com"), 443)]));
    assert!(idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .is_some());
    assert!(idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .is_none());
}

#[test]
fn test_atomic_listener_update_hostnames_changed() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1")],
        "default",
    );
    let mut gateway = gateway(serde_json::json!([listener("a", Some("old.example.com"), 443)]));
    idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
    gateway.spec.listeners.as_mut().unwrap()[0].hostname = Some("new.example.com".into());
    let routes = idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .unwrap();
    assert_eq!(routes, [(ResourceKind::HTTPRoute, "default/r1".to_string())]);
}

#[test]
fn test_atomic_listener_update_ports_changed() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(ResourceKind::TLSRoute, "default/t1", &[pr("default", "gw1")], "default");
    let mut gateway = gateway(serde_json::json!([listener("a", Some("a.example.com"), 443)]));
    idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
    gateway.spec.listeners.as_mut().unwrap()[0].port = 8443;
    let routes = idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .unwrap();
    assert_eq!(routes, [(ResourceKind::TLSRoute, "default/t1".to_string())]);
}

#[test]
fn test_atomic_listener_update_first_call_returns_some() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1")],
        "default",
    );
    let gateway = gateway(serde_json::json!([listener("a", None, 443)]));
    let routes = idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .unwrap();
    assert_eq!(routes, [(ResourceKind::HTTPRoute, "default/r1".to_string())]);
    assert!(idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .is_none());
}

#[test]
fn test_atomic_listener_update_no_referencing_routes() {
    let idx = GatewayRouteIndex::new();
    let gateway = gateway(serde_json::json!([]));
    assert_eq!(
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway),
        Some(vec![])
    );
    assert!(idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .is_none());
}

#[test]
fn update_route_returns_dropped_gateways() {
    let index = GatewayRouteIndex::new();

    let dropped = index.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw-a")],
        "default",
    );
    assert!(dropped.is_empty(), "first registration drops nothing");

    // Pre-register before lookup without dropping old edges before status.
    index.register_route_dependencies(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw-b")],
        "default",
    );
    assert_eq!(index.get_routes_for_gateway("default/gw-a").len(), 1);
    assert_eq!(index.get_routes_for_gateway("default/gw-b").len(), 1);
    // Re-point after status: gw-a must still be reported dropped.
    let dropped = index.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw-b")],
        "default",
    );
    assert_eq!(dropped, vec!["default/gw-a".to_string()]);
    assert!(index.get_routes_for_gateway("default/gw-a").is_empty());

    // Unchanged refs drop nothing.
    let dropped = index.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw-b")],
        "default",
    );
    assert!(dropped.is_empty());
}

#[test]
fn test_atomic_listener_update_input_order_irrelevant() {
    let idx = GatewayRouteIndex::new();
    let mut gateway = gateway(serde_json::json!([
        listener("a", Some("a.example.com"), 443),
        listener("b", Some("b.example.com"), 8443)
    ]));
    idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
    gateway.spec.listeners.as_mut().unwrap().reverse();
    assert!(idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .is_none());
}

#[test]
fn attachment_inputs_preserve_associations_and_policy_changes() {
    let base = serde_json::json!([listener("a", Some("a.example.com"), 443), listener("b", None, 443)]);
    for (pointer, value) in [
        ("/0/name", serde_json::json!("renamed")),
        ("/0/protocol", serde_json::json!("tls")),
        ("/0/port", serde_json::json!(443 + 65536)),
        (
            "/0/allowedRoutes",
            serde_json::json!({"kinds": [{"group": "other.test", "kind": "TLSRoute"}]}),
        ),
        (
            "/0/allowedRoutes",
            serde_json::json!({"namespaces": {"from": "Selector", "selector": {"matchLabels": {"team": "blue"}}}}),
        ),
    ] {
        let idx = GatewayRouteIndex::new();
        let original = gateway(base.clone());
        idx.update_listener_config_and_collect_routes("default/gw1", &original);
        let mut changed = base.clone();
        if pointer.ends_with("allowedRoutes") {
            changed[0]["allowedRoutes"] = value;
        } else {
            *changed.pointer_mut(pointer).unwrap() = value;
        }
        assert!(
            idx.update_listener_config_and_collect_routes("default/gw1", &gateway(changed))
                .is_some(),
            "{pointer}"
        );
    }
    let idx = GatewayRouteIndex::new();
    let mut gateway = gateway(base);
    idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
    let listeners = gateway.spec.listeners.as_mut().unwrap();
    listeners[1].hostname = listeners[0].hostname.take();
    assert!(idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .is_some());
    gateway.spec.addresses = Some(vec![serde_json::from_value(
        serde_json::json!({"type": "IPAddress", "value": "127.0.0.1"}),
    )
    .unwrap()]);
    assert!(idx
        .update_listener_config_and_collect_routes("default/gw1", &gateway)
        .is_some());
}

#[test]
fn removing_listener_config_keeps_edges_for_recreation_and_clear_resets_all() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(ResourceKind::TLSRoute, "default/r1", &[pr("default", "gw1")], "default");
    let gateway = gateway(serde_json::json!([listener("a", None, 443)]));
    idx.update_listener_config_and_collect_routes("default/gw1", &gateway);
    let removed = idx.remove_listener_config_and_collect_routes("default/gw1");
    assert_eq!(removed, [(ResourceKind::TLSRoute, "default/r1".to_string())]);
    assert_eq!(
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway),
        Some(removed)
    );
    idx.clear();
    assert!(idx.get_routes_for_gateway("default/gw1").is_empty());
    assert_eq!(
        idx.update_listener_config_and_collect_routes("default/gw1", &gateway),
        Some(vec![])
    );
}

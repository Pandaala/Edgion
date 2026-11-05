use super::*;
use serde_json::{json, Value};

fn gateway() -> Gateway {
    crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"name": "gateway", "namespace": "apps"},
        "spec": {"gatewayClassName": "public", "listeners": [
            {"name": "web", "port": 8080, "protocol": "HTTP", "hostname": "example.com"}
        ]}
    }))
    .unwrap()
}

fn route(kind: &str, rules: Value) -> Value {
    json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": kind,
        "metadata": {"name": "route", "namespace": "apps"},
        "spec": {"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "gateway", "sectionName": "web"}],
            "resolvedListeners": ["0.0.0.0:8080/HTTP"], "rules": rules}
    })
}

fn extension(name: &str) -> Value {
    json!({"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": name}})
}

#[test]
fn gateway_projection_preserves_annotation_and_serving_listener_contracts() {
    let mut gateway = gateway();
    gateway.metadata.annotations = Some(std::collections::BTreeMap::from([
        (
            annotations::EDGION_PLUGINS.to_owned(),
            "local shared/plugin local".to_owned(),
        ),
        (
            annotations::EDGION_STREAM_PLUGINS.to_owned(),
            " shared/connection ".to_owned(),
        ),
    ]));
    let input = GatewayInput::from_gateway(&gateway);
    assert_eq!(input.class_name, "public");
    assert_eq!(input.http_keys, ["apps/local", "shared/plugin"]);
    assert_eq!(input.connection_key.as_deref(), Some(""));
    assert_eq!(input.listeners[0].hostname.as_deref(), Some("example.com"));
    assert_eq!(input.listeners[0].key, ListenerKey("0.0.0.0:8080/HTTP".to_owned()));

    for (value, expected) in [(" local ", Some("apps/local")), ("  ", None)] {
        gateway
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(annotations::EDGION_STREAM_PLUGINS.to_owned(), value.to_owned());
        assert_eq!(GatewayInput::from_gateway(&gateway).connection_key.as_deref(), expected);
    }
    for (condition, status) in [("Conflicted", "True"), ("Programmed", "False"), ("Accepted", "False")] {
        gateway.spec.current_status = Some(serde_json::from_value(json!({"listeners": [{
                "name": "web", "supportedKinds": [], "attachedRoutes": 0,
                "conditions": [{"type": condition, "status": status, "reason": "Test", "message": "", "lastTransitionTime": "2026-09-11T00:00:00Z"}]
            }]})).unwrap());
        assert!(GatewayInput::from_gateway(&gateway).listeners.is_empty(), "{condition}");
    }
}

#[test]
fn http_input_preserves_source_identity_and_owner_extension_namespaces() {
    // An injected `resolvedNamespace` is not a field: ExtensionRefs resolve in
    // the declaring route's namespace.
    let mut original = extension("original");
    original["extensionRef"]["resolvedNamespace"] = json!("child");
    let raw = route(
        "HTTPRoute",
        json!([{
            "filters": [original],
            "backendRefs": [{"name": "backend", "port": 80, "filters": [extension("backend-plugin")]}]
        }]),
    );
    let source: Arc<HTTPRoute> = Arc::new(crate::test_current_status::from_value(raw).unwrap());
    let input = RouteInput::from_http(Arc::clone(&source));
    assert!(Arc::ptr_eq(input.http_route.as_ref().unwrap(), &source));
    assert_eq!(
        input.http_keys,
        HashSet::from(["apps/original".to_owned(), "apps/backend-plugin".to_owned()])
    );
    assert_eq!(
        effective_http_rules(&source)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        [0]
    );
}

#[test]
fn http_demand_skips_invalid_rules_native_backend_cors_and_unsupported_refs() {
    let mut wrong_kind = extension("wrong-kind");
    wrong_kind["extensionRef"]["kind"] = json!("Secret");
    let mut wrong_group = extension("wrong-group");
    wrong_group["extensionRef"]["group"] = json!("other.io");
    let mut empty_group = extension("empty-group");
    empty_group["extensionRef"]["group"] = json!("");
    let mut raw = route(
        "HTTPRoute",
        json!([
            {"filters": [extension("invalid")]},
            {"filters": [extension("cors-invalid")], "backendRefs": [{"name": "backend", "port": 80, "filters": [{"type": "CORS", "cors": {}}]}]},
            {"filters": [extension("valid"), wrong_kind, wrong_group, empty_group]}
        ]),
    );
    raw["spec"]["invalidRuleIndices"] = json!([0]);
    let source = Arc::new(crate::test_current_status::from_value::<HTTPRoute>(raw).unwrap());
    assert_eq!(
        effective_http_rules(&source)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        [2]
    );
    let input = RouteInput::from_http(source);
    assert_eq!(input.http_keys, HashSet::from(["apps/valid".to_owned()]));
}

#[test]
fn grpc_demand_skips_invalid_rules_and_reads_backend_filters() {
    let mut raw = route("GRPCRoute", json!([{"filters": [extension("original")]}]));
    raw["spec"]["invalidRuleIndices"] = json!([0]);
    let mut source: GRPCRoute = crate::test_current_status::from_value(raw).unwrap();
    assert!(RouteInput::from_grpc(&source).http_keys.is_empty());
    source.spec.invalid_rule_indices = None;
    source.spec.rules = Some(
        serde_json::from_value(json!([{
            "filters": [extension("delegated")],
            "backendRefs": [{"name": "backend", "port": 80, "filters": [extension("backend-plugin")]}]
        }]))
        .unwrap(),
    );
    let input = RouteInput::from_grpc(&source);
    assert_eq!(
        input.http_keys,
        HashSet::from(["apps/delegated".to_owned(), "apps/backend-plugin".to_owned()])
    );
    assert!(input.http_route.is_none());
}

#[test]
fn accepted_parents_keep_order_and_attach_only_to_allowed_serving_listeners() {
    let mut raw = route("HTTPRoute", json!([{}]));
    raw["spec"]["parentRefs"] = json!([
        {"name": "other", "kind": "Service"},
        {"name": "denied"},
        {"name": "gateway", "sectionName": "web"},
        {"name": "second"}
    ]);
    let mut source: HTTPRoute = crate::test_current_status::from_value(raw).unwrap();
    source.spec.current_status.as_mut().unwrap().parents[1].conditions[0].status = "False".into();
    let mut input = RouteInput::from_http(Arc::new(source));
    assert_eq!(
        input
            .parents
            .iter()
            .map(|parent| parent.gateway_key.as_str())
            .collect::<Vec<_>>(),
        ["apps/gateway", "apps/second"]
    );
    let mut gateway = GatewayInput::from_gateway(&gateway());
    assert!(input.attaches_to("apps/gateway", &gateway));
    assert!(!input.attaches_to("apps/other", &gateway));
    gateway.listeners[0].name = "other".to_owned();
    assert!(!input.attaches_to("apps/gateway", &gateway));
    gateway.listeners[0].name = "web".to_owned();
    input.namespace = "tenant".to_owned();
    assert!(
        !input.attaches_to("apps/gateway", &gateway),
        "default namespace policy is Same"
    );
    gateway.listeners[0].allowed_routes =
        Some(serde_json::from_value(json!({"namespaces": {"from": "All"}, "kinds": [{"kind": "HTTPRoute"}]})).unwrap());
    assert!(input.attaches_to("apps/gateway", &gateway));
    input.kind = RouteKind::Grpc;
    assert!(!input.attaches_to("apps/gateway", &gateway));
    input.kind = RouteKind::Http;
    input.listeners.clear();
    assert!(!input.attaches_to("apps/gateway", &gateway));
}

#[test]
fn stream_routes_select_their_existing_stage_and_annotation_contract() {
    let mut raw = route("TCPRoute", json!([{}]));
    raw["metadata"]["annotations"] = json!({annotations::EDGION_STREAM_PLUGINS: " local "});
    let tcp = RouteInput::from_tcp(&serde_json::from_value(raw.clone()).unwrap());
    assert_eq!(tcp.stream_key.as_deref(), Some("apps/local"));
    assert!(tcp.stages.connection && !tcp.stages.tls_route);
    raw["kind"] = json!("UDPRoute");
    let udp = RouteInput::from_udp(&serde_json::from_value(raw.clone()).unwrap());
    assert_eq!(udp.stream_key, tcp.stream_key);
    assert_eq!(udp.stages, tcp.stages);
    raw["kind"] = json!("TLSRoute");
    raw["spec"]["hostnames"] = json!(["api.example.com"]);
    raw["spec"]["resolvedListeners"] = json!([{
        "listenerKey": "0.0.0.0:8443/TLS", "gatewayNamespace": "apps",
        "gatewayName": "gateway", "listenerName": "web", "hostnames": ["api.example.com"]
    }]);
    raw["metadata"]["annotations"] = json!({annotations::EDGION_STREAM_PLUGINS: " shared/policy "});
    let tls = RouteInput::from_tls(&crate::test_current_status::from_value(raw).unwrap());
    assert_eq!(tls.stream_key.as_deref(), Some("shared/policy"));
    assert!(!tls.stages.connection && tls.stages.tls_route);
    assert!(tls.http_keys.is_empty() && tls.http_route.is_none());
}

#[test]
fn owning_reference_demand_rejects_invalid_sources_names_and_stamps_per_descriptor() {
    for kind in ["HTTPRoute", "GRPCRoute"] {
        for (field, value) in [
            ("name", ""),
            ("name", "bad/name"),
            ("group", ""),
            ("group", "core"),
            ("kind", "Secret"),
        ] {
            let mut invalid = extension("decoy");
            invalid["extensionRef"][field] = json!(value);
            let raw = route(
                kind,
                json!([{ "filters": [invalid, extension("valid")], "backendRefs":[{"name":"service","port":80,"filters":[extension("backend")]}] }]),
            );
            let keys = if kind == "HTTPRoute" {
                RouteInput::from_http(Arc::new(crate::test_current_status::from_value(raw).unwrap())).http_keys
            } else {
                RouteInput::from_grpc(&crate::test_current_status::from_value(raw).unwrap()).http_keys
            };
            assert_eq!(
                keys,
                HashSet::from(["apps/valid".into(), "apps/backend".into()]),
                "{kind} {field}"
            );
        }
        let mut raw = route(kind, json!([{ "filters": [extension("valid")] }]));
        raw["metadata"]["name"] = json!("bad/name");
        let keys = if kind == "HTTPRoute" {
            RouteInput::from_http(Arc::new(crate::test_current_status::from_value(raw).unwrap())).http_keys
        } else {
            RouteInput::from_grpc(&crate::test_current_status::from_value(raw).unwrap()).http_keys
        };
        assert!(keys.is_empty());
    }
}

#[test]
fn actual_gateway_attachment_projection_rejects_invalid_slots_and_owner() {
    let mut owner = gateway();
    owner.metadata.namespace = None;
    owner.metadata.annotations = Some(std::collections::BTreeMap::from([(
        annotations::EDGION_PLUGINS.into(),
        "local, default/local /bad other/good bad/a/b missing".into(),
    )]));
    assert_eq!(
        GatewayInput::from_gateway(&owner).http_keys,
        ["default/local", "other/good", "default/missing"]
    );
    assert!(owner.metadata.namespace.is_none());
    owner.metadata.name = None;
    assert!(GatewayInput::from_gateway(&owner).http_keys.is_empty());
    for value in ["", " , \t,\n "] {
        owner
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(annotations::EDGION_PLUGINS.into(), value.into());
        assert!(GatewayInput::from_gateway(&owner).http_keys.is_empty());
    }
}

#[test]
fn configured_stream_gateway_owner_errors_remain_present_and_blank_remains_absent() {
    let mut owner = gateway();
    owner.metadata.annotations = Some(std::collections::BTreeMap::from([(
        annotations::EDGION_STREAM_PLUGINS.into(),
        "stream".into(),
    )]));
    owner.metadata.namespace = None;
    assert_eq!(
        GatewayInput::from_gateway(&owner).connection_key.as_deref(),
        Some("default/stream")
    );
    owner.metadata.name = None;
    assert_eq!(GatewayInput::from_gateway(&owner).connection_key.as_deref(), Some(""));
    owner
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(annotations::EDGION_STREAM_PLUGINS.into(), " \t ".into());
    assert_eq!(GatewayInput::from_gateway(&owner).connection_key, None);
}

#[test]
fn fixed_link_sys_source_updates_publish_new_views_and_keep_captured_generations() {
    use crate::plugins::publication::PluginPublisher;
    let publisher = PluginPublisher::new();
    let class: edgion_resources::GatewayClass = serde_json::from_value(json!({
        "metadata":{"name":"public"},"spec":{"controllerName":"edgion.io/gateway-controller"}
    }))
    .unwrap();
    publisher.replace_gateway_classes(std::collections::HashMap::from([("public".into(), Arc::new(class))]));
    let mut gateway = gateway();
    gateway.metadata.annotations = Some(std::collections::BTreeMap::from([(
        "edgion.io/edgion-plugins".into(),
        "fixed-links".into(),
    )]));
    publisher.replace_gateways(std::collections::HashMap::from([(
        "apps/gateway".into(),
        GatewayInput::from_gateway(&gateway),
    )]));
    let mut owner: edgion_resources::EdgionPlugins = serde_json::from_value(json!({
        "metadata":{"name":"fixed-links","namespace":"apps"},"spec":{"upstreamResponseFilterPlugins":[{
            "type":"Dsl","config":{"name":"program","source":"let x = 1","linkSysRefs":[{"namespace":"apps","name":"first"}]}
        }]}
    })).unwrap();
    publisher.replace_http_plugins(std::collections::HashMap::from([(
        "apps/fixed-links".into(),
        Arc::new(owner.clone()),
    )]));
    let captured = publisher.snapshot().gateway("apps/gateway").unwrap().scope.clone();
    let first = captured.http("apps/fixed-links").unwrap();
    assert!(first.is_activatable, "{:?}", first.preparse_errors);
    let edgion_resources::resources::edgion_plugins::EdgionPlugin::Dsl(config) =
        &mut owner.spec.upstream_response_filter_plugins.as_mut().unwrap()[0].plugin
    else {
        unreachable!()
    };
    config.linksys_refs[0].name = "second".into();
    publisher.replace_http_plugins(std::collections::HashMap::from([(
        "apps/fixed-links".into(),
        Arc::new(owner),
    )]));
    let next = publisher
        .snapshot()
        .gateway("apps/gateway")
        .unwrap()
        .scope
        .http("apps/fixed-links")
        .unwrap();
    assert!(next.is_activatable);
    assert!(!Arc::ptr_eq(&first.plugin_runtime, &next.plugin_runtime));
    publisher.replace_http_plugins(std::collections::HashMap::new());
    assert!(publisher
        .snapshot()
        .gateway("apps/gateway")
        .unwrap()
        .scope
        .http("apps/fixed-links")
        .is_none());
    assert!(Arc::ptr_eq(&first, &captured.http("apps/fixed-links").unwrap()));
}

#[test]
fn delegation_child_flag_follows_accepted_delegation_keys() {
    use edgion_resources::resources::route_delegation::{
        key_for, DelegationTarget, DelegationTargetName, RouteKindName,
    };
    let mut child: HTTPRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "child", "namespace": "apps"},
        "spec": {"resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "parent"}],
            "rules": [{"filters": [extension("plugins")], "backendRefs": [{"name": "svc", "port": 80}]}]}
    }))
    .unwrap();
    assert!(
        !RouteInput::from_http(Arc::new(child.clone())).delegation_child,
        "no Controller-confirmed key: not published as a child"
    );
    child.spec.resolved_delegation_keys = vec![key_for(
        RouteKindName::HTTPRoute,
        "apps",
        "parent",
        &DelegationTarget {
            ns: "apps".into(),
            name: DelegationTargetName::Named("child".into()),
        },
    )];
    let input = RouteInput::from_http(Arc::new(child));
    assert!(input.delegation_child);
    assert!(input.http_keys.contains("apps/plugins"));
    assert!(input.listeners.is_empty());

    let ordinary: HTTPRoute = crate::test_current_status::from_value(route(
        "HTTPRoute",
        json!([{"filters": [extension("plugins")], "backendRefs": [{"name": "svc", "port": 80}]}]),
    ))
    .unwrap();
    assert!(!RouteInput::from_http(Arc::new(ordinary)).delegation_child);
}

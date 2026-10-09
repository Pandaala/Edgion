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
fn http_delegation_preserves_source_identity_and_resolved_extension_namespaces() {
    let mut raw = route("HTTPRoute", json!([{"filters": [extension("original")]}]));
    let mut delegated = extension("delegated");
    delegated["extensionRef"]["resolvedNamespace"] = json!("child");
    raw["spec"]["invalidRuleIndices"] = json!([0]);
    raw["spec"]["resolvedRules"] = json!([{
        "filters": [delegated],
        "backendRefs": [{"name": "backend", "port": 80, "filters": [extension("backend-plugin")]}]
    }]);
    let source: Arc<HTTPRoute> = Arc::new(crate::test_current_status::from_value(raw).unwrap());
    let input = RouteInput::from_http(Arc::clone(&source));
    assert!(Arc::ptr_eq(input.http_route.as_ref().unwrap(), &source));
    assert_eq!(
        input.http_keys,
        HashSet::from(["child/delegated".to_owned(), "apps/backend-plugin".to_owned()])
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
fn grpc_demand_uses_effective_rules_and_backend_filters() {
    let mut raw = route("GRPCRoute", json!([{"filters": [extension("original")]}]));
    raw["spec"]["invalidRuleIndices"] = json!([0]);
    let mut source: GRPCRoute = crate::test_current_status::from_value(raw).unwrap();
    assert!(RouteInput::from_grpc(&source).http_keys.is_empty());
    let mut delegated = extension("delegated");
    delegated["extensionRef"]["resolvedNamespace"] = json!("child");
    source.spec.resolved_rules = Some(
        serde_json::from_value(json!([{
            "filters": [delegated],
            "backendRefs": [{"name": "backend", "port": 80, "filters": [extension("backend-plugin")]}]
        }]))
        .unwrap(),
    );
    let input = RouteInput::from_grpc(&source);
    assert_eq!(
        input.http_keys,
        HashSet::from(["child/delegated".to_owned(), "apps/backend-plugin".to_owned()])
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
            ("resolvedNamespace", "UPPER"),
        ] {
            let mut invalid = extension("decoy");
            invalid["extensionRef"][field] = json!(value);
            let mut raw = route(
                kind,
                json!([{ "filters": [invalid, extension("valid")], "backendRefs":[{"name":"service","port":80,"filters":[extension("backend")]}] }]),
            );
            raw["spec"]["resolvedRules"] = raw["spec"]["rules"].take();
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

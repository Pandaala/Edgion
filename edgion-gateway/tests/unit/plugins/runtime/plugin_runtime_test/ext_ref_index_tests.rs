use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::{ForwardAuthConfig, GRPCRouteFilter, LocalObjectReference};

async fn assert_generic_config_rejection(runtime: &PluginRuntime) {
    assert!(runtime.rejects_invalid_config());
    let filter = runtime.request_plugins_iter().next().expect("rejection filter");
    assert_eq!(filter.name(), "RejectInvalidConfig");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new(filter.name());
    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: Some(bytes::Bytes::from_static(
                br#"{"message":"plugin configuration error"}"#
            )),
        }
    );
    let rendered = log.render_for_test();
    assert!(
        rendered.contains("ExtensionRef"),
        "missing static attribution: {rendered}"
    );
}

fn make_ext_ref_filter(name: &str) -> HTTPRouteFilter {
    make_ext_ref_filter_with_kind("EdgionPlugins", name)
}

fn make_ext_ref_filter_with_kind(kind: &str, name: &str) -> HTTPRouteFilter {
    HTTPRouteFilter {
        filter_type: HTTPRouteFilterType::ExtensionRef,
        request_header_modifier: None,
        response_header_modifier: None,
        request_redirect: None,
        url_rewrite: None,
        request_mirror: None,
        cors: None,
        external_auth: None,
        extension_ref: Some(LocalObjectReference {
            group: "edgion.io".into(),
            kind: kind.into(),
            name: name.into(),
            resolved_namespace: None,
        }),
    }
}

fn make_external_auth_filter(config: ForwardAuthConfig) -> HTTPRouteFilter {
    HTTPRouteFilter {
        filter_type: HTTPRouteFilterType::ExternalAuth,
        request_header_modifier: None,
        response_header_modifier: None,
        request_redirect: None,
        url_rewrite: None,
        request_mirror: None,
        cors: None,
        external_auth: Some(edgion_resources::HTTPRouteExternalAuth::Edgion(Box::new(config))),
        extension_ref: None,
    }
}

#[test]
fn invalid_external_auth_seals_rule_and_backend_runtimes() {
    // The default ForwardAuth target is intentionally invalid: neither a URL
    // nor a Service name is configured, so ForwardAuth::new must fail.
    let filters = vec![make_external_auth_filter(ForwardAuthConfig::default())];

    let runtimes = [
        owned_http_rule(&filters, "ns1", &MirrorTuning::default()),
        owned_http_backend(&filters, "ns1", &MirrorTuning::default()),
    ];

    for runtime in runtimes {
        assert_eq!(runtime.request_plugins_count(), 1);
        assert_eq!(
            runtime.request_plugins_iter().next().map(|filter| filter.name()),
            Some("RejectInvalidConfig")
        );
    }
}

#[tokio::test]
async fn external_auth_response_header_policy_preserves_native_runtime_defense() {
    for header in ["CoNtEnT-LeNgTh", "WWW-Authenticate", "Set-Cookie", "Content-Encoding"] {
        let config: ForwardAuthConfig = serde_json::from_value(serde_json::json!({
            "target": {"name": "auth", "port": 8080},
            "decision": {"clientHeaders": [header]}
        }))
        .unwrap();
        let filters = vec![make_external_auth_filter(config)];
        for runtime in [
            owned_http_rule(&filters, "ns1", &MirrorTuning::default()),
            owned_http_backend(&filters, "ns1", &MirrorTuning::default()),
        ] {
            assert_eq!(runtime.request_plugins_count(), 1);
            assert_eq!(
                runtime.request_plugins_iter().next().map(|filter| filter.name()),
                Some(if header == "CoNtEnT-LeNgTh" {
                    "RejectInvalidConfig"
                } else {
                    "ForwardAuth"
                })
            );
        }
    }
}

#[tokio::test]
async fn missing_extension_ref_payload_fails_closed_in_all_route_scopes() {
    let http: Vec<HTTPRouteFilter> = serde_json::from_value(serde_json::json!([
        {"type": "ExtensionRef"}
    ]))
    .expect("HTTP filter fixture must deserialize");
    let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(serde_json::json!([
        {"type": "ExtensionRef"}
    ]))
    .expect("gRPC filter fixture must deserialize");

    for runtime in [
        owned_http_rule(&http, "default", &MirrorTuning::default()),
        owned_http_backend(&http, "default", &MirrorTuning::default()),
        owned_grpc_rule(&grpc, "default"),
        owned_grpc_backend(&grpc, "default"),
    ] {
        assert_generic_config_rejection(&runtime).await;
    }
}

#[test]
fn rule_scope_assigns_sequential_indices() {
    let filters = vec![make_ext_ref_filter("p1"), make_ext_ref_filter("p2")];
    let rt = owned_http_rule(&filters, "ns1", &MirrorTuning::default());
    let keys = rt.ext_ref_keys();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], Some("ns1/p1".to_string()));
    assert_eq!(keys[1], Some("ns1/p2".to_string()));
}

#[test]
fn backend_scope_does_not_collect_keys() {
    let filters = vec![make_ext_ref_filter("p1")];
    let rt = owned_http_backend(&filters, "ns1", &MirrorTuning::default());
    assert!(rt.ext_ref_keys().is_empty());
}

#[test]
fn grpc_rule_scope_uses_controller_resolved_namespace() {
    let filter: GRPCRouteFilter = serde_json::from_value(serde_json::json!({
        "type": "ExtensionRef",
        "extensionRef": {
            "group": "edgion.io",
            "kind": "EdgionPlugins",
            "name": "child-plugin",
            "resolvedNamespace": "child"
        }
    }))
    .expect("gRPC ExtensionRef fixture must deserialize");
    let mut route: edgion_resources::GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "parent", "namespace": "parent"}, "spec": {"rules": [{"filters": [filter]}]}
    }))
    .unwrap();
    route.spec.resolved_rules = route.spec.rules.take();
    let runtime = PluginRuntime::from_grpcroute_filters_for_rule(&route, 0);
    assert_eq!(runtime.ext_ref_keys(), &[Some("child/child-plugin".to_string())]);
}

#[test]
fn lb_policy_kind_extension_ref_now_builds_a_filter_like_any_other_kind() {
    // The route-level extensionRef LB mechanism was removed: `LBPolicy*`
    // kinds are no longer special-cased by the plugin runtime. A
    // `LBPolicyConsistentHash` ExtensionRef is now treated exactly like any
    // other ExtensionRef filter, so it builds an ExtensionRefFilter.
    // (Routes using this kind are excluded from the data-plane route table
    // upstream at the controller's extensionRef-kind validation gate, so
    // this filter would never actually execute in production — but the
    // gateway-side runtime construction no longer knows or cares about the
    // kind.)
    let filters = vec![make_ext_ref_filter_with_kind(
        "LBPolicyConsistentHash",
        "header.x-user-id",
    )];
    let rt = owned_http_backend(&filters, "ns1", &MirrorTuning::default());
    assert_eq!(
        rt.request_plugins_count(),
        1,
        "LBPolicy* kinds are no longer special-cased; an ExtensionRefFilter is built like any other kind"
    );
}

fn owned_http_rule(
    filters: &[edgion_resources::HTTPRouteFilter],
    namespace: &str,
    tuning: &MirrorTuning,
) -> PluginRuntime {
    let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "route", "namespace": namespace}, "spec": {"rules": [{"filters": filters}]}
    }))
    .unwrap();
    PluginRuntime::from_httproute_filters_for_rule(&route, 0, tuning)
}

fn owned_http_backend(
    filters: &[edgion_resources::HTTPRouteFilter],
    namespace: &str,
    tuning: &MirrorTuning,
) -> PluginRuntime {
    let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "route", "namespace": namespace}, "spec": {"rules": [{"backendRefs": [{"name": "service", "port": 80, "filters": filters}]}]}
    })).unwrap();
    PluginRuntime::from_httproute_filters_for_backend(&route, 0, 0, tuning)
}

fn owned_grpc_rule(filters: &[edgion_resources::GRPCRouteFilter], namespace: &str) -> PluginRuntime {
    let route: edgion_resources::GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "route", "namespace": namespace}, "spec": {"rules": [{"filters": filters}]}
    }))
    .unwrap();
    PluginRuntime::from_grpcroute_filters_for_rule(&route, 0)
}

fn owned_grpc_backend(filters: &[edgion_resources::GRPCRouteFilter], namespace: &str) -> PluginRuntime {
    let route: edgion_resources::GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "route", "namespace": namespace}, "spec": {"rules": [{"backendRefs": [{"name": "service", "port": 80, "filters": filters}]}]}
    })).unwrap();
    PluginRuntime::from_grpcroute_filters_for_backend(&route, 0, 0)
}

#[test]
fn owning_extension_slots_validate_targets_and_effective_locality() {
    let base = serde_json::json!({"metadata":{"name":"route"}, "spec":{"rules":[{"filters":[
        {"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"first","resolvedNamespace":"child"}},
        {"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"bad/name"}},
        {"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"last"}}
    ]}]}});
    for grpc in [false, true] {
        for resolved in [false, true] {
            let mut value = base.clone();
            if resolved {
                value["spec"]["resolvedRules"] = value["spec"]["rules"].take();
            }
            let runtime = if grpc {
                PluginRuntime::from_grpcroute_filters_for_rule(&serde_json::from_value(value).unwrap(), 0)
            } else {
                PluginRuntime::from_httproute_filters_for_rule(
                    &serde_json::from_value(value).unwrap(),
                    0,
                    &MirrorTuning::default(),
                )
            };
            assert_eq!(
                runtime.ext_ref_keys(),
                &[
                    Some(if resolved { "child/first" } else { "default/first" }.into()),
                    None,
                    Some("default/last".into())
                ]
            );
            assert_eq!(runtime.request_plugins_count(), 3);
            assert_eq!(runtime.upstream_response_plugins_count(), 3);
            assert_eq!(runtime.upstream_response_body_plugins_count(), 3);
            assert_eq!(runtime.upstream_response_async_plugins_count(), 3);
            assert!(runtime.invalid_extension_gate.is_some());
            assert!(!runtime.rejects_invalid_config());
        }
        for (field, value) in [
            ("name", serde_json::json!("")),
            ("name", serde_json::json!("bad/name")),
            ("group", serde_json::json!("")),
            ("group", serde_json::json!("core")),
            ("kind", serde_json::json!("Service")),
            ("resolvedNamespace", serde_json::json!("UPPER")),
        ] {
            let mut input = base.clone();
            input["spec"]["rules"][0]["filters"][0]["extensionRef"][field] = value;
            if field == "resolvedNamespace" {
                input["spec"]["resolvedRules"] = input["spec"]["rules"].take();
            }
            let runtime = if grpc {
                PluginRuntime::from_grpcroute_filters_for_rule(&serde_json::from_value(input).unwrap(), 0)
            } else {
                PluginRuntime::from_httproute_filters_for_rule(
                    &serde_json::from_value(input).unwrap(),
                    0,
                    &MirrorTuning::default(),
                )
            };
            assert!(runtime.ext_ref_keys()[0].is_none());
        }
        for (field, value) in [("name", ""), ("name", "bad/name"), ("namespace", "UPPER")] {
            let mut input = base.clone();
            input["metadata"][field] = serde_json::json!(value);
            let runtime = if grpc {
                PluginRuntime::from_grpcroute_filters_for_rule(&serde_json::from_value(input).unwrap(), 0)
            } else {
                PluginRuntime::from_httproute_filters_for_rule(
                    &serde_json::from_value(input).unwrap(),
                    0,
                    &MirrorTuning::default(),
                )
            };
            assert!(runtime.rejects_invalid_config());
            assert!(runtime.ext_ref_keys().is_empty());
        }
    }
    let route: edgion_resources::HTTPRoute = serde_json::from_value(base).unwrap();
    assert!(
        PluginRuntime::from_httproute_filters_for_rule(&route, 9, &MirrorTuning::default()).rejects_invalid_config()
    );
    assert!(
        PluginRuntime::from_httproute_filters_for_backend(&route, 0, 9, &MirrorTuning::default())
            .rejects_invalid_config()
    );
}

#[tokio::test]
async fn invalid_effective_backend_extension_rejects_without_any_target_query() {
    for grpc in [false, true] {
        for (field, value) in [
            ("name", ""),
            ("name", "bad/name"),
            ("group", ""),
            ("kind", "Secret"),
            ("resolvedNamespace", "UPPER"),
        ] {
            let mut reference = serde_json::json!({"group":"edgion.io","kind":"EdgionPlugins","name":"decoy"});
            reference[field] = serde_json::json!(value);
            let source = serde_json::json!({"metadata":{"name":"route","namespace":"default"},"spec":{"resolvedRules":[{"backendRefs":[{"name":"service","port":80,"filters":[{"type":"ExtensionRef","extensionRef":reference}]}]}]}});
            let runtime = if grpc {
                PluginRuntime::from_grpcroute_filters_for_backend(&serde_json::from_value(source).unwrap(), 0, 0)
            } else {
                PluginRuntime::from_httproute_filters_for_backend(
                    &serde_json::from_value(source).unwrap(),
                    0,
                    0,
                    &MirrorTuning::default(),
                )
            };
            assert!(runtime.ext_ref_keys().is_empty());
            assert!(runtime.invalid_extension_gate.is_none());
            assert!(!runtime.rejects_invalid_config());
            assert_eq!(runtime.request_plugins_count(), 1);
            let mut session = MockPluginSession::new();
            session.expect_resolved_route_ext_ref().never();
            session.expect_pinned_backend_ext_ref().never();
            session.expect_write_response_header().once().returning(|header, _| {
                assert_eq!(header.status.as_u16(), 500);
                Ok(())
            });
            session.expect_write_response_body().once().returning(|_, _| Ok(()));
            let filter = runtime.request_plugins_iter().next().unwrap();
            let mut log = PluginLog::new(filter.name());
            assert_eq!(
                filter.run_request(&mut session, &mut log).await,
                PluginRunningResult::ErrTerminateRequest
            );
            assert!(log.render_for_test().contains("extension ref target not supported"));
        }
    }
}

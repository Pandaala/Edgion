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
        PluginRuntime::from_httproute_filters_for_rule(&filters, "ns1", &MirrorTuning::default()),
        PluginRuntime::from_httproute_filters_for_backend(&filters, "ns1", &MirrorTuning::default()),
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
            PluginRuntime::from_httproute_filters_for_rule(&filters, "ns1", &MirrorTuning::default()),
            PluginRuntime::from_httproute_filters_for_backend(&filters, "ns1", &MirrorTuning::default()),
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
        PluginRuntime::from_httproute_filters_for_rule(&http, "default", &MirrorTuning::default()),
        PluginRuntime::from_httproute_filters_for_backend(&http, "default", &MirrorTuning::default()),
        PluginRuntime::from_grpcroute_filters_for_rule(&grpc, "default"),
        PluginRuntime::from_grpcroute_filters_for_backend(&grpc, "default"),
    ] {
        assert_generic_config_rejection(&runtime).await;
    }
}

#[test]
fn rule_scope_assigns_sequential_indices() {
    let filters = vec![make_ext_ref_filter("p1"), make_ext_ref_filter("p2")];
    let rt = PluginRuntime::from_httproute_filters_for_rule(&filters, "ns1", &MirrorTuning::default());
    let keys = rt.ext_ref_keys();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], ("ns1".to_string(), "p1".to_string()));
    assert_eq!(keys[1], ("ns1".to_string(), "p2".to_string()));
}

#[test]
fn backend_scope_does_not_collect_keys() {
    let filters = vec![make_ext_ref_filter("p1")];
    let rt = PluginRuntime::from_httproute_filters_for_backend(&filters, "ns1", &MirrorTuning::default());
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
    let runtime = PluginRuntime::from_grpcroute_filters_for_rule(&[filter], "parent");
    assert_eq!(
        runtime.ext_ref_keys(),
        &[("child".to_string(), "child-plugin".to_string())]
    );
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
    let rt = PluginRuntime::from_httproute_filters_for_backend(&filters, "ns1", &MirrorTuning::default());
    assert_eq!(
        rt.request_plugins_count(),
        1,
        "LBPolicy* kinds are no longer special-cased; an ExtensionRefFilter is built like any other kind"
    );
}

use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use serde_json::json;

async fn assert_rejected(runtime: &PluginRuntime) {
    assert!(runtime.rejects_invalid_config());
    let filter = runtime.request_plugins_iter().next().expect("rejection filter");
    assert_eq!(filter.name(), "RejectInvalidConfig");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new(filter.name());
    let result = filter.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: Some(bytes::Bytes::from_static(
                br#"{"message":"plugin configuration error"}"#
            )),
        }
    );
    assert!(!log.render_for_test().contains("private-value"));
}

#[tokio::test]
async fn header_modifier_invalid_native_lists_seal_all_four_constructors() {
    for filters in [
        json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
            "set": [{"name": "Content-Length", "value": "123"}]
        }}]),
        json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
            "remove": ["Proxy-Connection"]
        }}]),
        json!([{"type": "RequestHeaderModifier"}]),
        json!([{"type": "ResponseHeaderModifier"}]),
        json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {
            "set": [{"name": "bad name", "value": "private-value"}]
        }}]),
        json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
            "add": [{"name": "x-test", "value": "private-value\r\ninvalid"}]
        }}]),
        json!([{"type": "RequestHeaderModifier", "requestHeaderModifier": {
            "remove": ["bad name"]
        }}]),
        json!([{"type": "RequestHeaderModifier", "responseHeaderModifier": {}}]),
        json!([
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {}},
            {"type": "RequestHeaderModifier", "requestHeaderModifier": {}}
        ]),
        json!([{"type": "ResponseHeaderModifier", "responseHeaderModifier": {
            "set": [{"name": "X-Same", "value": "ok"}], "remove": ["x-same"]
        }}]),
    ] {
        let http: Vec<HTTPRouteFilter> = serde_json::from_value(filters.clone()).unwrap();
        let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(filters).unwrap();
        let tuning = MirrorTuning::default();
        for runtime in [
            PluginRuntime::from_httproute_filters_for_rule(&http, "default", &tuning),
            PluginRuntime::from_httproute_filters_for_backend(&http, "default", &tuning),
            PluginRuntime::from_grpcroute_filters_for_rule(&grpc, "default"),
            PluginRuntime::from_grpcroute_filters_for_backend(&grpc, "default"),
        ] {
            assert_rejected(&runtime).await;
        }
    }
}

#[tokio::test]
async fn response_header_policy_schema_and_gateway_reject_the_same_plugin() {
    for name in ["Content-Length", "Proxy-Connection", "Connection"] {
        let plugin: EdgionPlugin = serde_json::from_value(json!({
            "type": "ResponseHeaderModifier", "config": {"remove": [name]}
        }))
        .unwrap();
        assert!(plugin.validation_error().is_some());
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_upstream_response_filters(&[UpstreamResponseFilterEntry::new(plugin)], "default");
        assert!(!errors.is_empty());
        runtime.seal_fail_closed();
        assert_rejected(&runtime).await;
    }
}

#[tokio::test]
async fn header_modifier_edgion_entries_reject_invalid_but_disabled_entries_are_inert() {
    let config: edgion_resources::HTTPHeaderFilter = serde_json::from_value(json!({
        "set": [{"name": "bad name", "value": "private-value"}]
    }))
    .unwrap();
    for enabled in [true, false] {
        let mut request = PluginRuntime::new();
        let errors = request.add_from_request_filters(
            &[RequestFilterEntry::with_enable(
                EdgionPlugin::RequestHeaderModifier(config.clone()),
                enabled,
            )],
            "default",
            "headers",
        );
        request.seal_fail_closed();
        assert_eq!(errors.is_empty(), !enabled);
        let mut response_entry = UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(config.clone()));
        response_entry.enable = enabled;
        let mut response = PluginRuntime::new();
        let errors = response.add_from_upstream_response_filters(&[response_entry], "default");
        response.seal_fail_closed();
        assert_eq!(errors.is_empty(), !enabled);
        for runtime in [request, response] {
            if enabled {
                assert_rejected(&runtime).await;
            } else {
                assert!(!runtime.has_invalid_plugins());
                assert_eq!(runtime.request_plugins_count(), 0);
            }
        }
    }
}

#[test]
fn header_modifier_valid_generation_keeps_native_operations() {
    let config = json!([
        {"type": "RequestHeaderModifier", "requestHeaderModifier": {
            "set": [{"name": "X-Set", "value": "ok"}]
        }},
        {"type": "ResponseHeaderModifier", "responseHeaderModifier": {
            "add": [{"name": "X-Add", "value": "ok"}]
        }}
    ]);
    let http: Vec<HTTPRouteFilter> = serde_json::from_value(config.clone()).unwrap();
    let grpc: Vec<GRPCRouteFilter> = serde_json::from_value(config).unwrap();
    for runtime in [
        PluginRuntime::from_httproute_filters_for_rule(&http, "default", &MirrorTuning::default()),
        PluginRuntime::from_httproute_filters_for_backend(&http, "default", &MirrorTuning::default()),
        PluginRuntime::from_grpcroute_filters_for_rule(&grpc, "default"),
        PluginRuntime::from_grpcroute_filters_for_backend(&grpc, "default"),
    ] {
        assert!(!runtime.has_invalid_plugins());
        assert_eq!(runtime.request_plugins_count(), 1);
        assert_eq!(
            runtime.request_plugins_iter().next().unwrap().name(),
            "RequestHeaderModifier"
        );
        assert_eq!(runtime.upstream_response_plugins.len(), 1);
    }
}

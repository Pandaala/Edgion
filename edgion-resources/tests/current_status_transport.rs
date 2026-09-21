//! Internal current-status transport stays outside every operator schema.
use edgion_resources::resources;
use serde_json::json;

#[test]
fn current_status_is_not_operator_schema_for_any_status_resource() {
    macro_rules! check {
        ($ty:ty) => {
            let schema = serde_json::to_value(schemars::schema_for!($ty)).unwrap();
            assert!(
                schema["properties"].get("currentStatus").is_none(),
                stringify!($ty)
            );
        };
    }
    check!(resources::http_route::HTTPRouteSpec);
    check!(resources::grpc_route::GRPCRouteSpec);
    check!(resources::tcp_route::TCPRouteSpec);
    check!(resources::tls_route::TLSRouteSpec);
    check!(resources::udp_route::UDPRouteSpec);
    check!(resources::gateway::GatewaySpec);
    check!(resources::gateway_class::GatewayClassSpec);
    check!(resources::edgion_tls::EdgionTlsSpec);
    check!(resources::backend_tls_policy::BackendTLSPolicySpec);
    check!(resources::edgion_backend_traffic_policy::EdgionBackendTrafficPolicySpec);
    check!(resources::edgion_plugins::EdgionPluginsSpec);
    check!(resources::edgion_stream_plugins::EdgionStreamPluginsSpec);
    check!(resources::edgion_backend::EdgionBackendSpec);
    check!(resources::link_sys::LinkSysSpec);
    check!(resources::edgion_gateway_config::EdgionGatewayConfigSpec);
    check!(resources::edgion_config_data::EdgionConfigDataSpec);
    check!(resources::edgion_acme::EdgionAcmeSpec);
}

#[test]
fn custom_deserializers_preserve_native_current_status_over_transport() {
    macro_rules! check {
        ($ty:ty, $source:expr) => {{
            let mut source = $source;
            let conditions = json!([{"type":"Accepted","status":"False","reason":"Invalid",
                "message":"Local validation failed","lastTransitionTime":"2026-09-21T00:00:00Z"}]);
            source["currentStatus"] = json!({"conditions": conditions});
            let spec: $ty = serde_json::from_value(source).unwrap();
            let serialized = serde_json::to_value(spec).unwrap();
            assert_eq!(serialized["currentStatus"]["conditions"], conditions);
            assert!(serialized["currentStatus"].get("controllers").is_none());
        }};
    }
    check!(resources::edgion_plugins::EdgionPluginsSpec, json!({}));
    check!(resources::edgion_stream_plugins::EdgionStreamPluginsSpec, json!({}));
    check!(
        resources::link_sys::LinkSysSpec,
        json!({"type":"redis", "config":{"endpoints":["redis://cache.example.com:6379"]}})
    );
    check!(
        resources::edgion_config_data::EdgionConfigDataSpec,
        json!({"data":{"type":"Misc","config":{}}})
    );
}

#[test]
fn listener_decisions_never_fall_back_to_full_status() {
    use resources::gateway::{gateway_listener_is_explicitly_ineligible, Gateway};
    let mut gateway: Gateway = serde_json::from_value(json!({
        "metadata":{"name":"gateway"},
        "spec":{"gatewayClassName":"class"},
        "status":{"listeners":[{"name":"http","attachedRoutes":0,"supportedKinds":[],
            "conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"",
                "lastTransitionTime":"2026-09-21T00:00:00Z"}]}]}
    }))
    .unwrap();
    assert!(gateway_listener_is_explicitly_ineligible(&gateway, "http"));
    gateway.spec.current_status = gateway.status.clone();
    assert!(!gateway_listener_is_explicitly_ineligible(&gateway, "http"));
    assert!(gateway_listener_is_explicitly_ineligible(&gateway, "other"));
    gateway
        .spec
        .current_status
        .as_mut()
        .unwrap()
        .listeners
        .as_mut()
        .unwrap()[0]
        .conditions[0]
        .status = "False".into();
    assert!(gateway_listener_is_explicitly_ineligible(&gateway, "http"));
}

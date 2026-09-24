//! Execute the shared minimal bundle fixtures through Edgion's source models.
//! Bundle-specific OpenAPI acceptance is checked by edgion-tests/gateway_api_bundles/check.py.

use edgion_resources::resources::*;
use edgion_resources::ResourceMeta;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

fn source<T: DeserializeOwned>(kind: &str, version: &str, fixtures: &Value) -> T {
    let spec = fixtures.get(kind).expect("fixture for every tested source kind");
    serde_json::from_value(json!({
        "apiVersion": format!("gateway.networking.k8s.io/{version}"),
        "kind": kind,
        "metadata": {"name": "fixture", "namespace": "default"},
        "spec": spec
    }))
    .unwrap_or_else(|error| panic!("{kind}/{version} did not decode into Edgion's source model: {error}"))
}

#[test]
fn canonical_minimal_fixtures_decode_and_preparse() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../edgion-tests/gateway_api_bundles/fixtures.json")).unwrap();
    let mut gateway_class: GatewayClass = source("GatewayClass", "v1", &fixtures);
    gateway_class.pre_parse();
    let mut gateway: Gateway = source("Gateway", "v1", &fixtures);
    gateway.pre_parse();
    let mut http: HTTPRoute = source("HTTPRoute", "v1", &fixtures);
    http.pre_parse();
    assert_eq!(http.effective_rules().unwrap().len(), 1);
    let mut grpc: GRPCRoute = source("GRPCRoute", "v1", &fixtures);
    grpc.pre_parse();
    assert_eq!(grpc.effective_rules().unwrap().len(), 1);
    let mut tls: TLSRoute = source("TLSRoute", "v1", &fixtures);
    tls.pre_parse();
    let mut grant: ReferenceGrant = source("ReferenceGrant", "v1", &fixtures);
    grant.pre_parse();
    let mut tcp: TCPRoute = source("TCPRoute", "v1", &fixtures);
    tcp.pre_parse();
    let mut udp: UDPRoute = source("UDPRoute", "v1", &fixtures);
    udp.pre_parse();
    let mut policy: BackendTLSPolicy = source("BackendTLSPolicy", "v1", &fixtures);
    policy.pre_parse();
}

#[test]
fn implemented_beta_sources_convert_to_canonical_models() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../edgion-tests/gateway_api_bundles/fixtures.json")).unwrap();
    let class: GatewayClass = source::<GatewayClassBeta>("GatewayClass", "v1beta1", &fixtures).into();
    assert_eq!(class.spec.controller_name, "example.com/gateway");
    let gateway: Gateway = source::<GatewayBeta>("Gateway", "v1beta1", &fixtures).into();
    assert_eq!(gateway.spec.gateway_class_name, "edge");
    let route: HTTPRoute = source::<HTTPRouteBeta>("HTTPRoute", "v1beta1", &fixtures).into();
    assert_eq!(route.effective_rules().unwrap().len(), 1);
    let grant: ReferenceGrant = source::<ReferenceGrantBeta>("ReferenceGrant", "v1beta1", &fixtures).into();
    assert_eq!(grant.spec.from.len(), 1);
}

#[test]
fn added_mirror_fraction_default_is_used_by_edgion_validation() {
    let mirror: HTTPRequestMirrorFilter = serde_json::from_value(json!({
        "backendRef": {"name": "backend", "port": 80},
        "fraction": {"numerator": 25}
    }))
    .unwrap();
    assert_eq!(mirror.fraction.as_ref().unwrap().denominator, None);
    assert!(mirror.validation_error().is_none());

    let too_large: HTTPRequestMirrorFilter = serde_json::from_value(json!({
        "backendRef": {"name": "backend", "port": 80},
        "fraction": {"numerator": 101}
    }))
    .unwrap();
    assert!(too_large.validation_error().unwrap().contains("between 0 and 100"));
}

#[test]
fn added_parent_port_and_loosened_route_bounds_decode() {
    let mut fixtures: Value =
        serde_json::from_str(include_str!("../../edgion-tests/gateway_api_bundles/fixtures.json")).unwrap();
    fixtures["HTTPRoute"]["parentRefs"][0]["port"] = json!(8080);
    fixtures["HTTPRoute"]["rules"][0]["matches"] = json!((0..9)
        .map(|_| json!({"path": {"type": "PathPrefix", "value": "/"}}))
        .collect::<Vec<_>>());
    let mut route: HTTPRoute = source("HTTPRoute", "v1", &fixtures);
    route.pre_parse();
    assert_eq!(route.spec.parent_refs.as_ref().unwrap()[0].port, Some(8080));
    assert_eq!(route.spec.rules.as_ref().unwrap()[0].matches.as_ref().unwrap().len(), 9);
    fixtures["TLSRoute"]["hostnames"] = json!((0..17).map(|index| format!("x{index}.example.com")).collect::<Vec<_>>());
    let mut tls: TLSRoute = source("TLSRoute", "v1", &fixtures);
    tls.pre_parse();
    assert_eq!(tls.spec.hostnames.as_ref().unwrap().len(), 17);
}

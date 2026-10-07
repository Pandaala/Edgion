use super::*;
use serde_json::json;

#[test]
fn defaults_resolve_against_the_owning_resource() {
    let omitted: ParentReference = serde_json::from_value(json!({"name": "gw"})).unwrap();
    let explicit: ParentReference = serde_json::from_value(json!({
        "name": "gw", "namespace": "apps", "kind": "Gateway", "group": GATEWAY_API_GROUP
    }))
    .unwrap();
    assert_eq!(
        omitted.canonical_identity(Some("apps")),
        explicit.canonical_identity(None)
    );
    assert_ne!(
        omitted.canonical_identity(Some("other")),
        explicit.canonical_identity(None)
    );
    assert_eq!(omitted.canonical_identity(None).namespace, "default");
    assert!(omitted.canonical_identity(None).is_gateway());
}

#[test]
fn every_explicit_attachment_dimension_and_owner_is_significant() {
    let base: ParentReference = serde_json::from_value(json!({"name": "gw"})).unwrap();
    for patch in [
        json!({"group": ""}),
        json!({"group": "example.net"}),
        json!({"kind": "HTTPRoute"}),
        json!({"namespace": "other"}),
        json!({"name": "other"}),
        json!({"sectionName": "https"}),
        json!({"port": 443}),
    ] {
        let mut value = json!({"name": "gw"});
        value
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let different: ParentReference = serde_json::from_value(value).unwrap();
        assert_ne!(
            base.canonical_identity(Some("apps")),
            different.canonical_identity(Some("apps"))
        );
    }
    let own = RouteParentStatus {
        parent_ref: base.clone(),
        controller_name: "custom.example/owner".into(),
        conditions: vec![],
    };
    let foreign = RouteParentStatus {
        parent_ref: base,
        controller_name: "other.example/owner".into(),
        conditions: vec![],
    };
    assert_ne!(own.canonical_identity(None), foreign.canonical_identity(None));
}

#[test]
fn processed_route_owner_is_transport_only_for_every_route_family() {
    use crate::resources::{
        grpc_route::GRPCRouteSpec, http_route::HTTPRouteSpec, tcp_route::TCPRouteSpec, tls_route::TLSRouteSpec,
        udp_route::UDPRouteSpec,
    };
    use schemars::{schema_for, JsonSchema};
    use serde::{de::DeserializeOwned, Serialize};

    fn check<T: DeserializeOwned + Serialize + JsonSchema>() {
        let input = json!({"parentRefs": [{"name": "gw"}], "resolvedStatusController": "custom.example/controller"});
        let spec: T = serde_json::from_value(input).unwrap();
        let wire = serde_json::to_value(&spec).unwrap();
        assert_eq!(wire["resolvedStatusController"], "custom.example/controller");
        let schema = serde_json::to_value(schema_for!(T)).unwrap();
        assert!(schema["properties"].get("resolvedStatusController").is_none());
        let authored: T = serde_json::from_value(json!({"parentRefs": [{"name": "gw"}]})).unwrap();
        assert!(serde_json::to_value(authored)
            .unwrap()
            .get("resolvedStatusController")
            .is_none());
    }
    check::<HTTPRouteSpec>();
    check::<GRPCRouteSpec>();
    check::<TCPRouteSpec>();
    check::<TLSRouteSpec>();
    check::<UDPRouteSpec>();
}

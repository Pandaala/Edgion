use super::*;

#[test]
fn standard_service_defaults_are_presence_aware_and_exact() {
    assert!(is_standard_service(None, None));
    assert!(is_standard_service(Some(""), Some("Service")));
    for group in ["core", "CORE", "Core", " core", "example.com"] {
        assert!(!is_standard_service(Some(group), None));
    }
    for kind in [
        "",
        "service",
        "ServiceClusterIp",
        "ServiceExternalName",
        "EndpointSlice",
        "ServiceEndpointSlice",
        "ServiceImport",
    ] {
        assert!(!is_standard_service(None, Some(kind)));
    }
    assert_eq!(backend_group(None), "");
    assert_eq!(backend_kind(Some("")), "");
}

#[test]
fn all_standard_backend_families_validate_before_object_use() {
    let value = serde_json::json!({"name":"api", "port":80});
    let http: HTTPBackendRef = serde_json::from_value(value.clone()).unwrap();
    let grpc: GRPCBackendRef = serde_json::from_value(value.clone()).unwrap();
    let tcp: TCPBackendRef = serde_json::from_value(value.clone()).unwrap();
    let tls: TLSBackendRef = serde_json::from_value(value.clone()).unwrap();
    let udp: UDPBackendRef = serde_json::from_value(value.clone()).unwrap();
    let mirror: BackendObjectReference = serde_json::from_value(value).unwrap();
    let identity = http.validated_target("routes").unwrap().object().unwrap();
    assert_eq!(identity, grpc.validated_target("routes").unwrap().object().unwrap());
    for target in [
        tcp.validated_target("routes"),
        tls.validated_target("routes"),
        udp.validated_target("routes"),
        mirror.validated_target("routes"),
    ] {
        assert_eq!(target.unwrap(), identity);
    }
    let mut http = http;
    for name in ["", "*", "bad/name", "UPPER"] {
        http.name = name.to_string();
        assert_eq!(http.validated_target("routes"), Err(ReferenceError::InvalidName));
    }
    http.name = "api".to_string();
    http.namespace = Some("".to_string());
    assert_eq!(http.validated_target("routes"), Err(ReferenceError::InvalidNamespace));
    http.namespace = None;
    for port in [None, Some(0), Some(-1), Some(65536)] {
        http.port = port;
        assert_eq!(http.validated_target("routes"), Err(ReferenceError::InvalidPort));
    }
}

#[test]
fn wildcard_delegation_is_a_selector_and_never_a_grant_object() {
    let reference: HTTPBackendRef = serde_json::from_value(serde_json::json!({
        "group":"gateway.networking.k8s.io", "kind":"HTTPRoute", "name":"*"
    }))
    .unwrap();
    let target = reference.validated_target("children").unwrap();
    assert!(target.object().is_none());
    let BackendTarget::NamespaceRoutes(selector) = target else {
        panic!("expected selector")
    };
    assert_eq!(selector.namespace(), "children");
    assert_eq!(selector.kind(), "HTTPRoute");
    assert!(reference.validated_target("").is_err());
}

#[test]
fn managed_http_service_identity_keeps_custom_modes_out_of_native_profiles() {
    use crate::resources::common::external_http::WebhookServiceRef;
    for group in [None, Some(""), Some("core")] {
        for kind in [
            None,
            Some(""),
            Some("Service"),
            Some("ServiceClusterIp"),
            Some("ServiceEndpointSlice"),
        ] {
            let mut wire = WebhookServiceRef {
                group: group.map(str::to_owned),
                kind: kind.map(str::to_owned),
                name: Some("auth".into()),
                port: Some(8080),
                ..Default::default()
            };
            let target = managed_http_service_target(&wire, "tenant").unwrap();
            assert_eq!(target.group_kind().group(), "");
            assert_eq!(target.group_kind().kind(), "Service");
            assert_eq!(target.scope().namespace(), Some("tenant"));
            wire.namespace = Some("shared".into());
            assert_eq!(
                managed_http_service_target(&wire, "tenant")
                    .unwrap()
                    .scope()
                    .namespace(),
                Some("shared")
            );
        }
    }
    for (group, kind, namespace, name, port) in [
        ("other.io", "Service", "tenant", "auth", 80),
        ("CORE", "Service", "tenant", "auth", 80),
        ("", "ServiceImport", "tenant", "auth", 80),
        ("", "ServiceExternalName", "tenant", "auth", 80),
        ("", "Service", "", "auth", 80),
        ("", "Service", "tenant", "bad/name", 80),
        ("", "Service", "tenant", "auth", 0),
    ] {
        let wire = WebhookServiceRef {
            group: Some(group.into()),
            kind: Some(kind.into()),
            namespace: Some(namespace.into()),
            name: Some(name.into()),
            port: Some(port),
        };
        assert!(managed_http_service_target(&wire, "tenant").is_err());
    }
    assert!(!is_standard_service(Some("core"), None));
    assert!(!is_standard_service(None, Some("ServiceClusterIp")));
}

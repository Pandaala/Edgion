use super::*;

#[test]
fn beta_conversion_preserves_rules_status_and_experimental_source_fields() {
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "route", "namespace": "app", "resourceVersion": "42"},
        "spec": {
            "parentRefs": [{"name": "edge"}], "hostnames": ["example.com"],
            "useDefaultGateways": "All",
            "rules": [{"matches": [{"method": "GET"}], "backendRefs": [{"name": "api", "port": 80}],
                "retry": {"codes": [503]},
                "filters": [{"type": "ExternalAuth", "externalAuth": {
                    "backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP",
                    "http": {"path": "/check"}
                }}]
            }]
        },
        "status": {"parents": [{"parentRef": {"name": "edge"}, "controllerName": "example.com/other", "conditions": []}]}
    });
    let beta: HTTPRouteBeta = serde_json::from_value(source.clone()).unwrap();
    let canonical: HTTPRoute = beta.into();
    assert_eq!(canonical.metadata.resource_version.as_deref(), Some("42"));
    assert_eq!(canonical.spec.use_default_gateways.as_deref(), Some("All"));
    assert_eq!(canonical.spec.hostnames.as_ref().unwrap(), &["example.com"]);
    assert_eq!(canonical.status.as_ref().unwrap().parents.len(), 1);
    let rule = &canonical.spec.rules.as_ref().unwrap()[0];
    assert_eq!(rule.backend_refs.as_ref().unwrap()[0].name, "api");
    assert!(rule.retry.is_some());
    assert_eq!(
        rule.matches.as_ref().unwrap()[0]
            .path
            .as_ref()
            .unwrap()
            .value
            .as_deref(),
        Some("/")
    );
    assert!(matches!(
        rule.filters.as_ref().unwrap()[0].external_auth,
        Some(HTTPRouteExternalAuth::Upstream(_))
    ));
}

#[test]
fn upstream_external_auth_shape_is_retained_for_rule_level_rejection() {
    for raw in [
        serde_json::json!({"backendRef": {"name": "auth", "port": 8080}, "protocol": "GRPC", "grpc": {}}),
        serde_json::json!({"backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP", "http": {"path": "/check"}}),
        serde_json::json!({"backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP",
                "http": {"path": "/check", "allowedHeaders": ["Authorization"]},
                "forwardBody": {"maxSize": 1024}}),
        serde_json::json!({"backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP",
                "http": {"path": "/check"}, "decision": {"clientHeaders": []}}),
    ] {
        let source: HTTPRouteExternalAuth = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(source, HTTPRouteExternalAuth::Upstream(_)));
        assert!(source.validation_error().unwrap().contains("not supported"));
        assert!(source.edgion_config().is_none());
        assert_eq!(serde_json::to_value(source).unwrap(), raw);
    }
}

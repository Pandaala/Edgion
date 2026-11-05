#[test]
fn hostname_list_validation_is_optional_bounded_and_strict() {
    for (hostnames, valid) in [
        (serde_json::Value::Null, true),
        (serde_json::json!([]), true),
        (serde_json::json!(vec!["a"; 16]), true),
        (serde_json::json!(vec!["a"; 17]), false),
        (serde_json::json!(["a", "*"]), false),
        (serde_json::json!(["127.0.0.1"]), false),
        (serde_json::json!(["*.127.0.0.1"]), true),
    ] {
        let spec: super::GRPCRouteSpec = serde_json::from_value(serde_json::json!({"hostnames": hostnames})).unwrap();
        assert_eq!(spec.validate_hostnames().is_ok(), valid);
    }
}
use super::*;

#[test]
fn method_match_compile_policy_survives_json_and_yaml_roundtrip() {
    for (source, valid) in [
        ("{}", true),
        (r#"{"method":{"method":"Get"}}"#, true),
        (r#"{"method":{"type":"Exact","method":"Get"}}"#, true),
        (r#"{"method":{"service":".pkg.Service_1"}}"#, true),
        (r#"{"method":{"service":"pkg.Service","method":"_Get2"}}"#, true),
        (r#"{"method":{}}"#, false),
        (r#"{"method":{"type":"Exact"}}"#, false),
        (r#"{"method":{"service":"","method":"Get"}}"#, false),
        (r#"{"method":{"service":"pkg.Service","method":""}}"#, false),
        (r#"{"method":{"service":"pkg..Service"}}"#, false),
        (r#"{"method":{"service":"pkg.Service."}}"#, false),
        (r#"{"method":{"method":"1Get"}}"#, false),
        (r#"{"method":{"method":"Get/Item"}}"#, false),
        (r#"{"method":{"method":"Gét"}}"#, false),
        (r#"{"method":{"type":"RegularExpression","method":"^Get.*$"}}"#, true),
        (
            r#"{"method":{"type":"RegularExpression","service":"","method":"Get"}}"#,
            true,
        ),
        (
            r#"{"method":{"type":"RegularExpression","service":"svc","method":""}}"#,
            true,
        ),
        (
            r#"{"method":{"type":"RegularExpression","service":"","method":""}}"#,
            false,
        ),
        (
            r#"{"method":{"type":"RegularExpression","service":"[","method":"Get"}}"#,
            false,
        ),
        (
            r#"{"method":{"type":"RegularExpression","service":"svc","method":"["}}"#,
            false,
        ),
    ] {
        let matcher: GRPCRouteMatch = serde_json::from_str(source).unwrap();
        assert_eq!(matcher.compile_method().is_ok(), valid, "{source}");
        let wire = serde_json::to_value(&matcher).unwrap();
        let json_copy: GRPCRouteMatch = serde_json::from_value(wire.clone()).unwrap();
        let yaml_copy: GRPCRouteMatch = yaml_serde::from_str(&yaml_serde::to_string(&matcher).unwrap()).unwrap();
        assert_eq!(json_copy.compile_method().is_ok(), valid, "{source}");
        assert_eq!(yaml_copy.compile_method().is_ok(), valid, "{source}");
        assert_eq!(serde_json::to_value(yaml_copy).unwrap(), wire);
    }
}

#[test]
fn compiled_method_distinguishes_any_exact_and_unconstrained_regex_fields() {
    let any: GRPCRouteMatch = serde_json::from_str("{}").unwrap();
    assert!(matches!(any.compile_method(), Ok(CompiledGrpcMethodMatch::Any)));
    let exact: GRPCRouteMatch = serde_json::from_str(r#"{"method":{"method":"Get"}}"#).unwrap();
    assert!(matches!(exact.compile_method(), Ok(CompiledGrpcMethodMatch::Exact)));
    let regex: GRPCRouteMatch =
        serde_json::from_str(r#"{"method":{"type":"RegularExpression","service":"","method":"^Get.*$"}}"#).unwrap();
    let CompiledGrpcMethodMatch::RegularExpression { service, method } = regex.compile_method().unwrap() else {
        panic!("expected regex matcher");
    };
    assert!(service.is_none());
    let method = method.unwrap();
    assert!(method.is_match("GetItem"));
    assert!(!method.is_match("DeleteItem"));
}

#[test]
fn exact_service_validation_matches_the_vendored_cel_case_fold_set() {
    for service in ["pkg.ſervice", "pkg.Kervice"] {
        let matcher: GRPCRouteMatch = serde_json::from_value(serde_json::json!({
            "method": {"service": service}
        }))
        .unwrap();
        assert!(matches!(matcher.compile_method(), Ok(CompiledGrpcMethodMatch::Exact)));
    }

    let rule: GRPCRouteRule = serde_json::from_value(serde_json::json!({
        "matches": [
            {"method": {"service": "pkg.Kervice"}},
            {"method": {"service": "pkg.Service"}}
        ]
    }))
    .unwrap();
    assert!(rule.method_match_error().is_none());
}

#[test]
fn header_matches_retain_the_first_ascii_case_equivalent_name() {
    let matcher: GRPCRouteMatch = serde_json::from_value(serde_json::json!({
        "headers": [
            {"name": "X-Mode", "type": "RegularExpression", "value": "^one$"},
            {"name": "x-mode", "value": "ignored"},
            {"name": "X-Region", "value": "west"},
            {"name": "x-region", "type": "RegularExpression", "value": "ignored"},
            {"name": "X-Final", "value": "yes"}
        ]
    }))
    .unwrap();

    let headers = matcher.headers.as_ref().unwrap();
    assert_eq!(headers.len(), 3);
    assert_eq!(
        (
            headers[0].name.as_str(),
            headers[0].match_type.as_deref(),
            headers[0].value.as_str()
        ),
        ("X-Mode", Some("RegularExpression"), "^one$")
    );
    assert_eq!(
        (headers[1].name.as_str(), headers[1].value.as_str()),
        ("X-Region", "west")
    );
    assert_eq!(
        (headers[2].name.as_str(), headers[2].value.as_str()),
        ("X-Final", "yes")
    );

    let absent: GRPCRouteMatch = serde_json::from_str("{}").unwrap();
    assert!(absent.headers.is_none());
    let empty: GRPCRouteMatch = serde_json::from_str(r#"{"headers":[]}"#).unwrap();
    assert!(empty.headers.as_ref().is_some_and(Vec::is_empty));
}

#[test]
fn method_match_length_and_rule_failure_boundaries() {
    for match_type in ["Exact", "RegularExpression"] {
        for (length, valid) in [(1024, true), (1025, false)] {
            let matcher: GRPCRouteMatch = serde_json::from_value(serde_json::json!({
                "method": {"type": match_type, "method": "a".repeat(length)}
            }))
            .unwrap();
            assert_eq!(matcher.compile_method().is_ok(), valid);
        }
    }
    let rule: GRPCRouteRule = serde_json::from_value(serde_json::json!({"matches": [
        {"method": {"method": "Get"}},
        {"method": {"type": "RegularExpression", "method": "private-marker["}}
    ]}))
    .unwrap();
    let error = rule.method_match_error().unwrap();
    assert_eq!(error, "gRPC method matcher contains an invalid regular expression");
    assert!(!error.contains("private-marker"));
    let catch_all: GRPCRouteRule = serde_json::from_str("{}").unwrap();
    assert!(catch_all.method_match_error().is_none());
}

fn backend_ref(group: Option<&str>, kind: Option<&str>) -> GRPCBackendRef {
    GRPCBackendRef {
        name: "target".to_string(),
        namespace: None,
        port: None,
        weight: None,
        group: group.map(str::to_string),
        kind: kind.map(str::to_string),
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    }
}

#[test]
fn test_is_route_delegation_ref_matrix() {
    // Positive: exact group + kind
    assert!(backend_ref(Some(GATEWAY_API_GROUP), Some("GRPCRoute")).is_route_delegation_ref());
    // Negative: kind absent (defaults to Service semantics)
    assert!(!backend_ref(Some(GATEWAY_API_GROUP), None).is_route_delegation_ref());
    // Negative: Service kind
    assert!(!backend_ref(None, Some("Service")).is_route_delegation_ref());
    // Negative: right kind, wrong/absent group (core group Service named GRPCRoute)
    assert!(!backend_ref(None, Some("GRPCRoute")).is_route_delegation_ref());
    assert!(!backend_ref(Some("example.io"), Some("GRPCRoute")).is_route_delegation_ref());
    // Negative: HTTPRoute is not a gRPC delegation target
    assert!(!backend_ref(Some(GATEWAY_API_GROUP), Some("HTTPRoute")).is_route_delegation_ref());
}

fn route_json(spec: serde_json::Value) -> GRPCRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {"name": "r", "namespace": "default"},
        "spec": spec
    }))
    .expect("route should deserialize")
}

#[test]
fn grpc_route_retry_accepts_the_i32_domain_ceiling() {
    let mut route = route_json(serde_json::json!({
        "rules": [{"retry": {"attempts": i32::MAX}}]
    }));

    route.parse_retry();

    assert_eq!(
        route.spec.rules.as_ref().unwrap()[0]
            .parsed_retry
            .as_ref()
            .and_then(|retry| retry.attempts),
        Some(i32::MAX as u32)
    );
}

#[test]
fn retry_non_idempotent_annotation_reaches_rules() {
    let mut route: GRPCRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": {
            "name": "parent", "namespace": "infra",
            "annotations": {"edgion.io/retry-non-idempotent": "true"}
        },
        "spec": {
            "rules": [{"matches": [{"method": {"service": "svc.Parent"}}]}]
        }
    }))
    .unwrap();

    route.preparse();

    let rule = &route.spec.rules.as_ref().unwrap()[0];
    assert_eq!(
        rule.parsed_allow_non_idempotent_retry,
        Some(true),
        "annotation copies must reach rules"
    );
}

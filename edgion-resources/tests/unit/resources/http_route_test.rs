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
        let spec: super::HTTPRouteSpec = serde_json::from_value(serde_json::json!({"hostnames": hostnames})).unwrap();
        assert_eq!(spec.validate_hostnames().is_ok(), valid);
    }
}
#[test]
fn header_matches_decode_first_equivalent_name_in_order() {
    for reverse in [false, true] {
        let mut duplicates = vec![
            serde_json::json!({"name": "X-Mode", "value": "one"}),
            serde_json::json!({"name": "x-mode", "type": "RegularExpression", "value": "^two$"}),
        ];
        if reverse {
            duplicates.reverse();
        }
        let first = duplicates[0].clone();
        duplicates.insert(1, serde_json::json!({"name": "X-Other", "value": "west"}));
        duplicates.push(serde_json::json!({"name": "X-Mode", "value": "ignored"}));
        let input = serde_json::json!({"headers": duplicates});
        let json: super::HTTPRouteMatch = serde_json::from_value(input.clone()).unwrap();
        let yaml: super::HTTPRouteMatch = serde_yaml::from_str(&serde_yaml::to_string(&input).unwrap()).unwrap();
        for parsed in [json, yaml] {
            let headers = parsed.headers.as_ref().unwrap();
            assert_eq!(headers.len(), 2);
            assert_eq!(serde_json::to_value(&headers[0]).unwrap(), first);
            assert_eq!(headers[1].name, "X-Other");
            let encoded = serde_json::to_value(&parsed).unwrap();
            let again: super::HTTPRouteMatch = serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(serde_json::to_value(again).unwrap(), encoded);
        }
    }
    for input in [serde_json::json!({}), serde_json::json!({"headers": null})] {
        let parsed: super::HTTPRouteMatch = serde_json::from_value(input).unwrap();
        assert!(parsed.headers.is_none());
    }
    let empty: super::HTTPRouteMatch = serde_json::from_value(serde_json::json!({"headers": []})).unwrap();
    assert!(empty.headers.unwrap().is_empty());
}

#[test]
fn header_matches_decode_independently_in_declared_and_resolved_rules() {
    let rules = serde_json::json!([{"matches": [
        {"headers": [{"name": "X-Mode", "value": "one"}, {"name": "x-mode", "value": "two"}]},
        {"headers": [{"name": "x-mode", "value": "two"}, {"name": "X-Mode", "value": "one"}]}
    ]}]);
    let route: super::HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "duplicates"},
        "spec": {"rules": rules, "resolvedRules": rules}
    }))
    .unwrap();
    for rules in [route.spec.rules, route.spec.resolved_rules] {
        let rules = rules.unwrap();
        let matches = rules[0].matches.as_ref().unwrap();
        for (index, expected) in ["one", "two"].into_iter().enumerate() {
            let headers = matches[index].headers.as_ref().unwrap();
            assert_eq!(headers.len(), 1);
            assert_eq!(headers[0].value, expected);
        }
    }
}

#[test]
fn credentialed_expose_headers_shared_validation() {
    for credentials in [
        serde_json::Value::Null,
        serde_json::json!(false),
        serde_json::json!(true),
    ] {
        for exposed in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!(["*"]),
            serde_json::json!(["X-Request-Id"]),
            serde_json::json!(["X-Request-Id", "*"]),
        ] {
            let config: super::HTTPCORSFilter = serde_json::from_value(serde_json::json!({
                "allowOrigins": ["https://app.example"], "allowCredentials": credentials,
                "exposeHeaders": exposed
            }))
            .unwrap();
            let invalid = credentials == serde_json::json!(true)
                && exposed
                    .as_array()
                    .is_some_and(|names| names.contains(&serde_json::json!("*")));
            assert_eq!(config.validate().is_err(), invalid);
            if invalid {
                assert!(config.validate().unwrap_err()[0].contains("use explicit header names"));
            }
        }
    }
}

fn cors_from(value: serde_json::Value) -> super::HTTPCORSFilter {
    serde_json::from_value(value).expect("CORS test fixture must deserialize")
}

#[test]
fn native_cors_max_age_defaults_and_int32_bounds_match_v15() {
    let omitted = cors_from(serde_json::json!({}));
    assert_eq!(omitted.max_age, 5);
    assert!(omitted.validate().is_ok());

    for max_age in [1, 3600, CORS_MAX_AGE] {
        let explicit = cors_from(serde_json::json!({"maxAge": max_age}));
        assert_eq!(explicit.max_age, max_age);
        assert!(explicit.validate().is_ok());
    }

    for max_age in [0, CORS_MAX_AGE + 1] {
        let invalid = cors_from(serde_json::json!({"maxAge": max_age}));
        assert!(invalid
            .validate()
            .unwrap_err()
            .iter()
            .any(|error| error.contains("maxAge")));
    }
}

#[test]
fn native_cors_origin_boundaries_match_v15_schema() {
    let longest_origin = format!("https://{}", "a".repeat(CORS_ORIGIN_MAX_LEN - "https://".len()));
    for origin in [
        "*".to_string(),
        "http://example.com:1".to_string(),
        "https://*.example.com:65535".to_string(),
        "https://*".to_string(),
        longest_origin,
    ] {
        let config = cors_from(serde_json::json!({"allowOrigins": [origin]}));
        assert!(config.validate().is_ok(), "origin should be valid: {config:?}");
    }

    for origin in [
        "",
        "ftp://example.com",
        "https://example.com/path",
        "https://bad_host.example",
        "https://example.com:0",
        "https://example.com:65536",
        "https://example.com:not-a-port",
        "https://*.*.example.com",
    ] {
        let config = cors_from(serde_json::json!({"allowOrigins": [origin]}));
        assert!(config.validate().is_err(), "origin should be invalid: {origin}");
    }
    let oversized = format!("https://{}", "a".repeat(CORS_ORIGIN_MAX_LEN - "https://".len() + 1));
    let config = cors_from(serde_json::json!({"allowOrigins": [oversized]}));
    assert!(config.validate().is_err());
}

#[test]
fn native_cors_lists_enforce_bounds_sets_and_wildcard_exclusivity() {
    let origins: Vec<String> = (0..CORS_LIST_MAX_ITEMS)
        .map(|index| format!("https://h{index}.example"))
        .collect();
    let headers: Vec<String> = (0..CORS_LIST_MAX_ITEMS)
        .map(|index| format!("X-Test-{index}"))
        .collect();
    let valid = cors_from(serde_json::json!({
        "allowOrigins": origins,
        "allowMethods": ["GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH"],
        "allowHeaders": headers.clone(),
        "exposeHeaders": headers,
    }));
    assert!(valid.validate().is_ok());

    for value in [
        serde_json::json!({"allowOrigins": ["*", "https://example.com"]}),
        serde_json::json!({"allowMethods": ["*", "GET"]}),
        serde_json::json!({"allowHeaders": ["*", "X-Test"]}),
        serde_json::json!({"allowOrigins": ["https://example.com", "https://example.com"]}),
        serde_json::json!({"allowMethods": ["GET", "GET"]}),
        serde_json::json!({"allowHeaders": ["X-Test", "X-Test"]}),
        serde_json::json!({"exposeHeaders": ["X-Test", "X-Test"]}),
    ] {
        assert!(cors_from(value).validate().is_err());
    }

    let too_many_origins: Vec<String> = (0..=CORS_LIST_MAX_ITEMS)
        .map(|index| format!("https://h{index}.example"))
        .collect();
    assert!(cors_from(serde_json::json!({"allowOrigins": too_many_origins}))
        .validate()
        .is_err());

    let too_many_headers: Vec<String> = (0..=CORS_LIST_MAX_ITEMS)
        .map(|index| format!("X-Test-{index}"))
        .collect();
    assert!(cors_from(serde_json::json!({"allowHeaders": too_many_headers.clone()}))
        .validate()
        .is_err());
    assert!(cors_from(serde_json::json!({"exposeHeaders": too_many_headers}))
        .validate()
        .is_err());
}

#[test]
fn native_cors_methods_and_header_names_match_v15_schema() {
    let maximum_header = "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize);
    assert!(cors_from(serde_json::json!({"allowHeaders": [maximum_header]}))
        .validate()
        .is_ok());
    for value in [
        serde_json::json!({"allowMethods": ["get"]}),
        serde_json::json!({"allowMethods": ["CUSTOM"]}),
        serde_json::json!({"allowHeaders": [""]}),
        serde_json::json!({"allowHeaders": ["bad header"]}),
        serde_json::json!({"exposeHeaders": [":method"]}),
        serde_json::json!({"allowHeaders": ["x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize + 1)]}),
    ] {
        assert!(cors_from(value).validate().is_err());
    }

    let wildcard_exposure = cors_from(serde_json::json!({
        "allowCredentials": false,
        "exposeHeaders": ["*", "X-Test"]
    }));
    assert!(wildcard_exposure.validate().is_ok());
}

#[test]
fn mirror_denial_is_transport_metadata_not_operator_schema() {
    let backend: super::BackendObjectReference = serde_json::from_value(serde_json::json!({
        "name": "shadow", "namespace": "target", "port": 80,
        "refDenied": {"targetNamespace": "target", "targetName": "shadow"}
    }))
    .unwrap();
    assert!(backend.is_service());
    let wire = serde_json::to_value(&backend).unwrap();
    assert_eq!(wire["refDenied"]["targetName"], "shadow");
    let restored: super::BackendObjectReference = serde_json::from_value(wire).unwrap();
    assert_eq!(restored.ref_denied, backend.ref_denied);
    let schema = serde_json::to_value(schemars::schema_for!(super::BackendObjectReference)).unwrap();
    assert!(schema["properties"].get("refDenied").is_none());
}

use super::*;
use std::time::Duration;

#[test]
fn backend_cors_scope_rejects_every_backend_policy() {
    use serde_json::json;
    let cors = json!({"type": "CORS", "cors": {"allowOrigins": ["https://app.example"]}});
    for filter in [cors.clone(), json!({"type": "CORS"})] {
        for weight in [0, 1, 50] {
            for rule_filters in [json!([]), json!([cors.clone()])] {
                let rule: HTTPRouteRule = serde_json::from_value(json!({
                    "filters": rule_filters,
                    "backendRefs": [
                        {"name": "plain", "port": 80},
                        {"name": "cors", "port": 80, "weight": weight, "filters": [filter]}
                    ]
                }))
                .unwrap();
                assert_eq!(
                    rule.unsupported_backend_cors_reason().as_deref(),
                    Some("backendRefs[1] CORS is not supported; configure CORS in rules[].filters instead")
                );
            }
        }
    }
    for rule in [
        json!({}),
        json!({"filters": [cors]}),
        json!({
            "backendRefs": [{"name": "plain", "port": 80, "filters": [
                {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-test", "value": "ok"}]}}
            ]}]
        }),
    ] {
        let rule: HTTPRouteRule = serde_json::from_value(rule).unwrap();
        assert_eq!(rule.unsupported_backend_cors_reason(), None);
    }
}

#[test]
fn repeated_rule_cors_is_rejected_by_v16_filter_validation() {
    let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "filters": [
            {"type": "CORS", "cors": {"allowOrigins": ["https://a.example"]}},
            {"type": "CORS", "cors": {"allowOrigins": ["https://b.example"]}}
        ]
    }))
    .unwrap();
    assert_eq!(
        rule.unsupported_cors_reason().as_deref(),
        Some("CORS filter cannot be repeated")
    );
}

#[test]
fn route_status_empty_parents_serialize_explicit_array() {
    // K8s status persistence uses JSON Merge Patch. An empty parents list
    // must serialize as `"parents": []`, not be omitted, or a merge patch
    // can never clear a previously-persisted non-empty list (reachable when
    // all parentRefs are removed from a live route). Covers every
    // route-family status struct that shares this parents shape.
    use crate::resources::edgion_tls::EdgionTlsStatus;
    use crate::resources::grpc_route::GRPCRouteStatus;
    use crate::resources::tcp_route::TCPRouteStatus;
    use crate::resources::tls_route::TLSRouteStatus;
    use crate::resources::udp_route::UDPRouteStatus;

    let expected = serde_json::json!({ "parents": [] });
    assert_eq!(serde_json::to_value(HTTPRouteStatus::default()).unwrap(), expected);
    assert_eq!(serde_json::to_value(GRPCRouteStatus::default()).unwrap(), expected);
    assert_eq!(serde_json::to_value(TCPRouteStatus::default()).unwrap(), expected);
    assert_eq!(serde_json::to_value(TLSRouteStatus::default()).unwrap(), expected);
    assert_eq!(serde_json::to_value(UDPRouteStatus::default()).unwrap(), expected);
    assert_eq!(serde_json::to_value(EdgionTlsStatus::default()).unwrap(), expected);
}

#[test]
fn match_without_path_defaults_to_path_prefix_root() {
    // The K8s API server backfills this from the structural schema at
    // admission; File/Etcd config-center sources never pass through it, so
    // the default must come from deserialization or the match compiles
    // into a route unit with no radix-tree entry (permanently unreachable).
    let m: HTTPRouteMatch = serde_json::from_value(serde_json::json!({
        "headers": [{ "name": "x-internal", "value": "true" }]
    }))
    .unwrap();
    let path = m.path.expect("omitted path must default, not stay None");
    assert_eq!(path.match_type.as_deref(), Some("PathPrefix"));
    assert_eq!(path.value.as_deref(), Some("/"));

    // An explicit path is never overwritten by the default.
    let m: HTTPRouteMatch = serde_json::from_value(serde_json::json!({
        "path": { "type": "Exact", "value": "/api" }
    }))
    .unwrap();
    let path = m.path.expect("explicit path must survive");
    assert_eq!(path.match_type.as_deref(), Some("Exact"));
    assert_eq!(path.value.as_deref(), Some("/api"));
}

fn backend_ref(group: Option<&str>, kind: Option<&str>) -> HTTPBackendRef {
    HTTPBackendRef {
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
fn backend_ref_classification_is_canonical() {
    for (group, kind) in [(None, None), (None, Some("Service")), (Some(""), Some("Service"))] {
        assert_eq!(backend_ref(group, kind).backend_kind(), HTTPBackendRefKind::Service);
    }

    assert_eq!(
        backend_ref(Some(EDGION_API_GROUP), Some("EdgionBackend")).backend_kind(),
        HTTPBackendRefKind::EdgionBackend
    );
    assert_eq!(
        backend_ref(Some(GATEWAY_API_GROUP), Some("HTTPRoute")).backend_kind(),
        HTTPBackendRefKind::RouteDelegation
    );

    for (group, kind) in [
        (None, Some("")),
        (Some("core"), Some("Service")),
        (None, Some("ServiceExternalName")),
        (None, Some("ServiceClusterIp")),
        (None, Some("ServiceImport")),
        (None, Some("ServiceEndpointSlice")),
        (None, Some("EndpointSlice")),
        (Some("core"), Some("ServiceExternalName")),
        (Some(""), Some("ServiceClusterIp")),
        (Some(EDGION_API_GROUP), None),
        (None, Some("EdgionBackend")),
        (Some("core"), Some("EdgionBackend")),
        (Some(GATEWAY_API_GROUP), Some("Service")),
        (Some(EDGION_API_GROUP), Some("HTTPRoute")),
        (Some("example.io"), Some("Widget")),
        // Pseudo-kinds also remain unsupported under extension groups.
        (Some(EDGION_API_GROUP), Some("ServiceExternalName")),
        (Some(GATEWAY_API_GROUP), Some("ServiceClusterIp")),
    ] {
        assert_eq!(backend_ref(group, kind).backend_kind(), HTTPBackendRefKind::Unsupported);
    }
}

#[test]
fn gep2257_basic_units() {
    assert_eq!(parse_gep2257_duration_secs("1h"), Some(3600));
    assert_eq!(parse_gep2257_duration_secs("5m"), Some(300));
    assert_eq!(parse_gep2257_duration_secs("30s"), Some(30));
    assert_eq!(parse_gep2257_duration_secs("2000ms"), Some(2));
}

#[test]
fn gep2257_compound_and_unordered() {
    assert_eq!(parse_gep2257_duration_secs("1h30m"), Some(5400));
    assert_eq!(parse_gep2257_duration_secs("30m1h"), Some(5400)); // unordered
    assert_eq!(parse_gep2257_duration_secs("1s500ms"), Some(2)); // 1500ms -> ceil 2
    assert_eq!(parse_gep2257_duration_secs("1h1h"), Some(7200)); // duplicate unit
}

#[test]
fn gep2257_ceil_min_one_second() {
    assert_eq!(parse_gep2257_duration_secs("500ms"), Some(1)); // ceil, floor 1
    assert_eq!(parse_gep2257_duration_secs("1ms"), Some(1));
    assert_eq!(parse_gep2257_duration_secs("1500ms"), Some(2));
}

#[test]
fn gep2257_rejects_non_conforming() {
    assert_eq!(parse_gep2257_duration_secs(""), None);
    assert_eq!(parse_gep2257_duration_secs("3600"), None); // bare integer not GEP-2257
    assert_eq!(parse_gep2257_duration_secs("1.5h"), None); // decimals not GEP-2257
    assert_eq!(parse_gep2257_duration_secs("1d"), None); // days not GEP-2257
    assert_eq!(parse_gep2257_duration_secs("1hour"), None); // long name not GEP-2257
    assert_eq!(parse_gep2257_duration_secs("abc"), None);
    assert_eq!(parse_gep2257_duration_secs("1h30x"), None); // trailing garbage
    assert_eq!(parse_gep2257_duration_secs("123456h"), None); // >5 digits per segment
    assert_eq!(parse_gep2257_duration_secs("1h1h1h1h1h"), None); // >4 segments
}

#[test]
fn sp_validate_ok_cases() {
    let unset = SessionPersistence {
        session_name: None,
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    assert!(unset.validate().is_ok());

    let permanent_ok = SessionPersistence {
        absolute_timeout: Some("1h".into()),
        cookie_config: Some(CookieConfig {
            lifetime_type: Some(CookieLifetimeType::Permanent),
        }),
        session_name: None,
        persistence_type: None,
        strict: None,
    };
    assert!(permanent_ok.validate().is_ok());
}

#[test]
fn sp_validate_permanent_requires_absolute_timeout() {
    let bad = SessionPersistence {
        absolute_timeout: None,
        cookie_config: Some(CookieConfig {
            lifetime_type: Some(CookieLifetimeType::Permanent),
        }),
        session_name: None,
        persistence_type: None,
        strict: None,
    };
    let err = bad.validate().unwrap_err();
    assert!(err.contains("absoluteTimeout"), "got: {err}");
}

#[test]
fn sp_validate_rejects_bad_duration() {
    let bad = SessionPersistence {
        absolute_timeout: Some("1d".into()), // not GEP-2257
        session_name: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    let err = bad.validate().unwrap_err();
    assert!(err.contains("absoluteTimeout"), "got: {err}");
}

#[test]
fn test_is_route_delegation_ref_matrix() {
    // Positive: exact group + kind
    assert!(backend_ref(Some(GATEWAY_API_GROUP), Some("HTTPRoute")).is_route_delegation_ref());
    // Negative: kind absent (defaults to Service semantics)
    assert!(!backend_ref(Some(GATEWAY_API_GROUP), None).is_route_delegation_ref());
    // Negative: Service kind
    assert!(!backend_ref(None, Some("Service")).is_route_delegation_ref());
    // Negative: right kind, wrong/absent group (core group Service named HTTPRoute)
    assert!(!backend_ref(None, Some("HTTPRoute")).is_route_delegation_ref());
    assert!(!backend_ref(Some("example.io"), Some("HTTPRoute")).is_route_delegation_ref());
    // Negative: GRPCRoute is not (yet) a delegation target
    assert!(!backend_ref(Some(GATEWAY_API_GROUP), Some("GRPCRoute")).is_route_delegation_ref());
}

fn route_json(spec: serde_json::Value) -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "r", "namespace": "default"},
        "spec": spec
    }))
    .expect("route should deserialize")
}

#[test]
fn request_mirror_filter_type_and_config_must_match() {
    let missing_config: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "filters": [{"type": "RequestMirror"}]
    }))
    .expect("rule should deserialize");
    assert_eq!(
        missing_config.request_mirror_validation_error().as_deref(),
        Some("filters[0] RequestMirror: requestMirror must be specified when filter.type is RequestMirror")
    );

    let unexpected_config: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "filters": [{
            "type": "URLRewrite",
            "requestMirror": {
                "backendRef": {"name": "mirror", "port": 8080}
            }
        }]
    }))
    .expect("rule should deserialize");
    assert_eq!(
        unexpected_config.request_mirror_validation_error().as_deref(),
        Some("filters[0] RequestMirror: requestMirror must be absent when filter.type is not RequestMirror")
    );
}

#[test]
fn test_effective_rules_prefers_resolved() {
    let mut route = route_json(serde_json::json!({
        "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/spec"}}]}]
    }));
    // No resolved_rules → falls back to spec.rules
    let rules = route.effective_rules().expect("spec rules present");
    assert_eq!(
        rules[0].matches.as_ref().unwrap()[0]
            .path
            .as_ref()
            .unwrap()
            .value
            .as_deref(),
        Some("/spec")
    );

    // With resolved_rules → resolved wins
    let resolved: Vec<HTTPRouteRule> = serde_json::from_value(serde_json::json!(
        [{"matches": [{"path": {"type": "PathPrefix", "value": "/resolved"}}]}]
    ))
    .unwrap();
    route.spec.resolved_rules = Some(resolved);
    let rules = route.effective_rules().expect("resolved rules present");
    assert_eq!(
        rules[0].matches.as_ref().unwrap()[0]
            .path
            .as_ref()
            .unwrap()
            .value
            .as_deref(),
        Some("/resolved")
    );
}

#[test]
fn test_resolved_rules_serialized_delegation_issues_not() {
    let mut route = route_json(serde_json::json!({"rules": []}));
    route.spec.resolved_rules = Some(Vec::new());
    route.spec.delegation_issues = vec![DelegationIssue::RuleDropped {
        message: "internal".to_string(),
    }];
    let json = serde_json::to_value(&route).unwrap();
    // resolved_rules must survive conf_sync serialization
    assert!(json["spec"].get("resolvedRules").is_some());
    // delegation_issues is controller-internal and must never serialize
    assert!(json["spec"].get("delegationIssues").is_none());
    assert!(json["spec"].get("delegation_issues").is_none());
}

#[test]
fn terminal_rule_identity_is_transport_only() {
    let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "name": "primary",
        "resolvedTerminalRouteUid": "route-uid",
        "resolvedTerminalRuleIdentity": "name:primary"
    }))
    .expect("rule with resolved terminal identity should deserialize");
    assert_eq!(rule.resolved_terminal_route_uid.as_deref(), Some("route-uid"));
    assert_eq!(rule.resolved_terminal_rule_identity.as_deref(), Some("name:primary"));

    let transported = serde_json::to_value(&rule).expect("resolved terminal identity should serialize");
    assert_eq!(transported["resolvedTerminalRouteUid"], "route-uid");
    assert_eq!(transported["resolvedTerminalRuleIdentity"], "name:primary");

    let schema = schemars::schema_for!(HTTPRouteRule);
    let schema_json = serde_json::to_value(schema).expect("HTTPRouteRule schema should serialize");
    let properties = schema_json["properties"]
        .as_object()
        .expect("HTTPRouteRule schema should have properties");
    assert!(!properties.contains_key("resolvedTerminalRouteUid"));
    assert!(!properties.contains_key("resolvedTerminalRuleIdentity"));
}

#[test]
fn test_backend_request_clamped_to_request() {
    let config = HTTPRouteTimeouts {
        request: Some("10s".to_string()),
        backend_request: Some("30s".to_string()),
    };
    let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
    assert_eq!(parsed.request_timeout, RouteTimeout::Duration(Duration::from_secs(10)));
    assert_eq!(
        parsed.backend_request_timeout,
        RouteTimeout::Duration(Duration::from_secs(10))
    );
}

#[test]
fn test_backend_request_within_limit() {
    let config = HTTPRouteTimeouts {
        request: Some("30s".to_string()),
        backend_request: Some("10s".to_string()),
    };
    let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
    assert_eq!(parsed.request_timeout, RouteTimeout::Duration(Duration::from_secs(30)));
    assert_eq!(
        parsed.backend_request_timeout,
        RouteTimeout::Duration(Duration::from_secs(10))
    );
}

#[test]
fn test_backend_request_only_no_clamp() {
    let config = HTTPRouteTimeouts {
        request: None,
        backend_request: Some("10s".to_string()),
    };
    let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
    assert_eq!(parsed.request_timeout, RouteTimeout::Unset);
    assert_eq!(
        parsed.backend_request_timeout,
        RouteTimeout::Duration(Duration::from_secs(10))
    );
}

#[test]
fn test_zero_timeout_means_disable() {
    // Gateway API spec: "0s" means "no timeout" (disable), not zero-second timeout
    let config = HTTPRouteTimeouts {
        request: Some("0s".to_string()),
        backend_request: Some("0s".to_string()),
    };
    let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
    assert_eq!(parsed.request_timeout, RouteTimeout::Disabled);
    assert_eq!(parsed.backend_request_timeout, RouteTimeout::Disabled);
}

#[test]
fn test_zero_request_with_nonzero_backend() {
    let config = HTTPRouteTimeouts {
        request: Some("0s".to_string()),
        backend_request: Some("10s".to_string()),
    };
    let parsed = ParsedRouteTimeouts::from_config(&config).unwrap();
    assert_eq!(parsed.request_timeout, RouteTimeout::Disabled);
    assert_eq!(
        parsed.backend_request_timeout,
        RouteTimeout::Duration(Duration::from_secs(10))
    );
}

#[test]
fn test_no_timeouts_returns_none() {
    let config = HTTPRouteTimeouts {
        request: None,
        backend_request: None,
    };
    assert!(ParsedRouteTimeouts::from_config(&config).is_none());
}

#[test]
fn request_mirror_lexical_namespaces_and_names_use_canonical_service_validation() {
    for backend in [
        serde_json::json!({"name":"bad/name","port":80}),
        serde_json::json!({"name":"copy","namespace":"bad/ns","port":80}),
    ] {
        let filter: HTTPRequestMirrorFilter =
            serde_json::from_value(serde_json::json!({"backendRef":backend})).unwrap();
        assert!(filter
            .validation_error()
            .unwrap()
            .contains("valid Kubernetes object names"));
    }
    let filter: HTTPRequestMirrorFilter =
        serde_json::from_value(serde_json::json!({"backendRef":{"name":"copy","group":"","port":80}})).unwrap();
    assert!(filter.validation_error().is_none());
}

#[test]
fn session_lifetime_projection_preserves_strictness_and_rounding() {
    use super::SessionPersistence;
    for (text, expected) in [
        ("0s", 1),
        ("0ms", 1),
        ("1ms", 1),
        ("1001ms", 2),
        ("1s1ms", 2),
        ("2s1h2s", 3604),
        ("99999h99999h99999h99999h", 1439985600),
    ] {
        let policy: SessionPersistence = serde_json::from_value(serde_json::json!({"absoluteTimeout": text})).unwrap();
        assert_eq!(policy.validated_lifetime_secs().unwrap(), Some(expected), "{text}");
        assert!(policy.validate().is_ok());
    }
    for text in ["", " 1s", "1s ", "\t1s", "1s\n", "1 s", "1d", "100000s", "1s1s1s1s1s"] {
        let policy: SessionPersistence = serde_json::from_value(serde_json::json!({"absoluteTimeout": text})).unwrap();
        assert!(policy.validated_lifetime_secs().is_err(), "{text:?}");
        assert!(policy.validate().is_err());
    }
    let absent: SessionPersistence = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(absent.validated_lifetime_secs().unwrap(), None);
    let permanent: SessionPersistence =
        serde_json::from_value(serde_json::json!({"cookieConfig": {"lifetimeType": "Permanent"}})).unwrap();
    assert_eq!(
        permanent.validated_lifetime_secs().unwrap_err(),
        "cookieConfig.lifetimeType 'Permanent' requires absoluteTimeout"
    );
}

#[test]
fn parent_match_transport_is_required_inside_variant_and_schema_hidden() {
    for value in [
        serde_json::json!({"kind": "direct"}),
        serde_json::json!({"kind": "delegated", "matches": [{"rawPathPrefixes": ["/api", "/api/v1"]}]}),
    ] {
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({"resolvedParentMatch": value})).unwrap();
        assert_eq!(
            serde_json::to_value(rule.clone()).unwrap()["resolvedParentMatch"],
            value
        );
    }
    for value in [
        serde_json::json!({"kind": "delegated"}),
        serde_json::json!({"kind": "delegated", "matches": [{}]}),
        serde_json::json!({"kind": "delegated", "matches": [{"pathPrefixes": ["/api"]}]}),
        serde_json::json!({"kind": "unknown"}),
        serde_json::json!({"kind": "direct", "matches": []}),
    ] {
        assert!(serde_json::from_value::<HTTPRouteRule>(serde_json::json!({"resolvedParentMatch": value})).is_err());
    }
    let schema = serde_json::to_value(schemars::schema_for!(HTTPRouteRule)).unwrap();
    assert!(!schema.to_string().contains("resolvedParentMatch"));
    let ordinary: HTTPRouteRule = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(ordinary.resolved_parent_match.is_none());
}

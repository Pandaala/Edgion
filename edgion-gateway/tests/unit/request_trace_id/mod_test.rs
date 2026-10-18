use super::*;
const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

fn rule(json: serde_json::Value) -> CompiledRequestIdRule {
    CompiledRequestIdRule::new(&serde_json::from_value(json).unwrap()).unwrap()
}
fn state() -> RequestTraceId {
    let mut state = RequestTraceId::default();
    let mut headers = HeaderMap::new();
    headers.insert("traceparent", HeaderValue::from_static(PARENT));
    state.initialize(&headers);
    state
}

#[test]
fn initializes_once_and_replaces_conflicting_default_with_w3c() {
    let mut state = RequestTraceId::default();
    assert!(state.is_empty());
    assert_eq!(serde_json::to_value(&state).unwrap(), serde_json::json!({}));
    let mut headers = HeaderMap::new();
    headers.insert("traceparent", HeaderValue::from_static(PARENT));
    headers.insert("x-trace-id", HeaderValue::from_static("legacy-123"));
    state.initialize(&headers);
    state.initialize(&HeaderMap::new());
    assert_eq!(state.correlation_id("X-Trace-Id"), state.standard_trace_id());
    assert_eq!(state.standard_trace_id(), Some("4bf92f3577b34da6a3ce929d0e0e4736"));
    assert_eq!(
        state.typed_trace_id().map(|id| lower_hex(&id)).as_deref(),
        state.standard_trace_id()
    );
    state.reconcile_request(&mut headers);
    assert_eq!(headers["traceparent"], PARENT);
    assert_eq!(headers[DEFAULT_HEADER], state.standard_trace_id().unwrap());
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::json!({"trace_id": state.standard_trace_id().unwrap()})
    );
}

#[test]
fn three_gateway_hops_preserve_w3c_and_first_custom_id() {
    for incoming_default in [None, Some("legacy-business-id")] {
        let mut wire = HeaderMap::new();
        wire.insert("traceparent", HeaderValue::from_static(PARENT));
        wire.insert("tracestate", HeaderValue::from_static("vendor=state"));
        if let Some(value) = incoming_default {
            wire.insert(DEFAULT_HEADER, HeaderValue::from_static(value));
        }
        let mut first_custom = None;
        for prefix in ["first-", "second-", "third-"] {
            let mut gateway = RequestTraceId::default();
            gateway.initialize(&wire);
            let custom = rule(serde_json::json!({
                "header": "x-order-id",
                "generator": {"type": "md5", "prefix": prefix}
            }));
            gateway.apply_rule(&custom, single_header(&wire, "x-order-id"), None);
            gateway.reconcile_request(&mut wire);
            let expected_custom = first_custom.get_or_insert_with(|| wire["x-order-id"].clone());
            assert!(expected_custom.to_str().unwrap().starts_with("first-"));
            assert_eq!(&wire["x-order-id"], expected_custom);
            assert_eq!(wire[DEFAULT_HEADER], "4bf92f3577b34da6a3ce929d0e0e4736");
            assert_eq!(wire["traceparent"], PARENT);
            assert_eq!(wire["tracestate"], "vendor=state");

            let mut response = HeaderMap::new();
            response.insert("x-order-id", HeaderValue::from_static("backend-stale"));
            gateway.reconcile_response(&mut response);
            assert_eq!(response["x-order-id"], wire["x-order-id"]);
            assert_eq!(response[DEFAULT_HEADER], wire[DEFAULT_HEADER]);
            assert!(!response.contains_key("traceparent"));
        }
    }
}

#[test]
fn invalid_or_duplicate_parent_restarts_and_drops_state() {
    for values in [vec!["bad"], vec![PARENT, PARENT]] {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append("traceparent", HeaderValue::from_str(value).unwrap());
        }
        headers.insert("tracestate", HeaderValue::from_static("vendor=value"));
        let mut state = RequestTraceId::default();
        state.initialize(&headers);
        state.reconcile_request(&mut headers);
        assert_ne!(headers["traceparent"], PARENT);
        assert!(w3c::parse_trace_id(headers["traceparent"].to_str().unwrap()).is_some());
        assert!(headers["traceparent"].to_str().unwrap().ends_with("-00"));
        assert!(!headers.contains_key("tracestate"));
    }
}

#[test]
fn disabled_default_suppresses_wire_headers_but_retains_trace_identity() {
    let mut state = state();
    let patch: DefaultIdConfig = serde_json::from_value(serde_json::json!({"enabled":false})).unwrap();
    state.apply_default_patch(&patch);
    let mut headers = HeaderMap::new();
    headers.insert("x-trace-id", HeaderValue::from_static("stale"));
    state.reconcile_response(&mut headers);
    assert!(!headers.contains_key("x-trace-id"));
    state.reconcile_request(&mut headers);
    assert!(!headers.contains_key(DEFAULT_HEADER));
    assert_eq!(headers["traceparent"], PARENT);
    assert_eq!(state.standard_trace_id(), Some("4bf92f3577b34da6a3ce929d0e0e4736"));
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::json!({"trace_id": state.standard_trace_id().unwrap()})
    );
    state.apply_default_patch(&patch);
    state.reconcile_response(&mut headers);
    assert!(!headers.contains_key(DEFAULT_HEADER));
    assert!(state.standard_trace_id().is_some());
}

#[test]
fn first_write_preserves_value_and_policy() {
    let mut state = state();
    let first = rule(serde_json::json!({"header":"x-order-id","echoResponse":false,"forwardUpstream":false}));
    let later = rule(serde_json::json!({"header":"X-Order-ID"}));
    state.apply_rule(&first, Some(HeaderValue::from_static("one")), None);
    assert_eq!(
        state.apply_rule(&later, Some(HeaderValue::from_static("two")), None),
        ApplyOutcome::Preserved
    );
    let mut headers = HeaderMap::new();
    headers.insert("x-order-id", HeaderValue::from_static("backend"));
    state.reconcile_response(&mut headers);
    assert!(!headers.contains_key("x-order-id"));
    state.reconcile_request(&mut headers);
    assert!(!headers.contains_key("x-order-id"));
    assert_eq!(state.correlation_id("x-order-id"), Some("one"));
}

#[test]
fn prefixes_apply_only_to_generated_values() {
    let rule = rule(serde_json::json!({"header":"x-order-id","generator":{"type":"md5","prefix":"order-"}}));
    let mut first = state();
    first.apply_rule(&rule, Some(HeaderValue::from_static("client")), None);
    assert_eq!(first.correlation_id("x-order-id"), Some("client"));
    let mut second = state();
    second.apply_rule(&rule, None, Some(HeaderValue::from_static("copied")));
    assert_eq!(second.correlation_id("x-order-id"), Some("copied"));
    let mut third = state();
    third.apply_rule(&rule, None, None);
    let generated = third.correlation_id("x-order-id").unwrap();
    assert!(generated.starts_with("order-"));
    assert_eq!(generated.len(), 38);
}

#[test]
fn rejects_ambiguous_sensitive_and_oversized_values() {
    let rule = rule(serde_json::json!({"header":"x-order-id","generateWhenMissing":false}));
    for value in ["", "a,b", "with space", &"a".repeat(257)] {
        let mut state = state();
        assert_eq!(
            state.apply_rule(&rule, Some(HeaderValue::from_str(value).unwrap()), None),
            ApplyOutcome::Absent
        );
    }
    let mut sensitive = HeaderValue::from_static("credential");
    sensitive.set_sensitive(true);
    let mut state = state();
    assert_eq!(state.apply_rule(&rule, None, Some(sensitive)), ApplyOutcome::Absent);
    let mut headers = HeaderMap::new();
    headers.append("x-order-id", HeaderValue::from_static("one"));
    headers.append("x-order-id", HeaderValue::from_static("one"));
    assert!(single_header(&headers, "x-order-id").is_none());
}

#[test]
fn composition_cap_also_bounds_absent_managed_headers() {
    let mut state = state();
    for index in 0..15 {
        let rule = rule(serde_json::json!({"header":format!("x-id-{index}"),"generateWhenMissing":false}));
        assert_eq!(state.apply_rule(&rule, None, None), ApplyOutcome::Absent);
    }
    let rule = rule(serde_json::json!({"header":"x-overflow"}));
    assert_eq!(state.apply_rule(&rule, None, None), ApplyOutcome::LimitExceeded);
    assert_eq!(state.correlation_ids.len(), 16);
}

#[test]
fn tracestate_grammar_and_combined_limits() {
    let mut headers = HeaderMap::new();
    headers.append("tracestate", HeaderValue::from_static("vendor=one"));
    headers.append("tracestate", HeaderValue::from_static("tenant@system=two"));
    assert_eq!(validated_tracestate(&headers).unwrap(), "vendor=one,tenant@system=two");
    for value in [
        "vendor=one,vendor=two",
        "A=bad",
        "vendor=bad=value",
        "vendor=",
        "vendor=value ",
    ] {
        headers.insert("tracestate", HeaderValue::from_str(value).unwrap());
        // A trailing OWS is permitted on the list member by the W3C grammar.
        if value.ends_with(' ') {
            assert!(validated_tracestate(&headers).is_some());
        } else {
            assert!(validated_tracestate(&headers).is_none(), "{value}");
        }
    }
    headers.insert(
        "tracestate",
        HeaderValue::from_str(&format!("vendor={}", "a".repeat(257))).unwrap(),
    );
    assert!(validated_tracestate(&headers).is_none());
}

#[test]
fn tracestate_accepts_empty_members_and_whitespace_header_lines() {
    for value in ["vendor=value,", "vendor=one,,other=two", ", \t,vendor=one, "] {
        let mut headers = HeaderMap::new();
        headers.insert("tracestate", HeaderValue::from_str(value).unwrap());
        assert_eq!(validated_tracestate(&headers).unwrap(), value);
        headers.append("tracestate", HeaderValue::from_static(" \t"));
        assert!(validated_tracestate(&headers).is_some());
    }
    let mut headers = HeaderMap::new();
    headers.insert("tracestate", HeaderValue::from_static(""));
    let mut state = RequestTraceId::default();
    state.initialize(&headers);
    assert!(state.typed_trace_id().is_some());
}

#[test]
fn serialization_separates_trace_id_and_custom_request_ids() {
    let mut state = state();
    let custom = rule(serde_json::json!({"header":"x-order-id","source":{"type":"traceId"}}));
    state.apply_rule(
        &custom,
        None,
        Some(HeaderValue::from_static("4bf92f3577b34da6a3ce929d0e0e4736")),
    );
    let serialized = serde_json::to_value(&state).unwrap();
    assert_eq!(
        serialized,
        serde_json::json!({
            "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
            "request_ids": {"x-order-id": "4bf92f3577b34da6a3ce929d0e0e4736"}
        })
    );
    assert_eq!(
        serde_json::to_string(&state).unwrap().matches("\"x-trace-id\"").count(),
        0
    );
}

#[test]
fn compiled_rule_rejects_reserved_default_name() {
    for header in ["x-trace-id", "X-Trace-ID"] {
        let config: RequestIdRule = serde_json::from_value(serde_json::json!({"header":header})).unwrap();
        assert!(CompiledRequestIdRule::new(&config).is_err());
    }
}

#[test]
fn default_patches_are_incremental_and_can_reenable() {
    let mut state = state();
    state.apply_default_patch(&DefaultIdConfig {
        enabled: Some(false),
        echo_response: Some(false),
        ..Default::default()
    });
    state.apply_default_patch(&DefaultIdConfig::default());
    assert!(state.correlation_id(DEFAULT_HEADER).is_none());
    state.apply_default_patch(&DefaultIdConfig {
        enabled: Some(true),
        ..Default::default()
    });
    assert_eq!(state.correlation_id(DEFAULT_HEADER), state.standard_trace_id());
    let mut headers = HeaderMap::new();
    state.reconcile_response(&mut headers);
    assert!(!headers.contains_key(DEFAULT_HEADER));
    state.reconcile_request(&mut headers);
    assert!(headers.contains_key(DEFAULT_HEADER));
}

#[test]
fn future_version_wire_representation_survives_reconciliation() {
    let future = "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-future";
    let mut headers = HeaderMap::new();
    headers.insert("traceparent", HeaderValue::from_static(future));
    let mut state = RequestTraceId::default();
    state.initialize(&headers);
    headers.insert("traceparent", HeaderValue::from_static("modified"));
    state.reconcile_request(&mut headers);
    assert_eq!(headers["traceparent"], future);
}

#[test]
fn all_generators_have_expected_shapes() {
    for (kind, length) in [
        ("uuidV4", 36),
        ("uuidV7", 36),
        ("randomHex", 32),
        ("nanoId", 21),
        ("md5", 32),
    ] {
        let config: RequestIdGenerator = serde_json::from_value(serde_json::json!({"type":kind})).unwrap();
        let one = generate(&config).unwrap();
        let two = generate(&config).unwrap();
        assert_eq!(one.as_bytes().len(), length);
        assert_ne!(one, two);
    }
}

#[test]
fn random_hex_runtime_bounds_and_prefix_are_preserved() {
    for count in [0, 7, 65, usize::MAX] {
        let config = RequestIdGenerator {
            kind: RequestIdGeneratorKind::RandomHex,
            bytes: Some(count),
            ..Default::default()
        };
        assert!(generate(&config).is_none());
    }
    for count in [8, 16, 64] {
        let config = RequestIdGenerator {
            kind: RequestIdGeneratorKind::RandomHex,
            prefix: "request-".into(),
            bytes: Some(count),
            ..Default::default()
        };
        let value = generate(&config).unwrap();
        let suffix = value.to_str().unwrap().strip_prefix("request-").unwrap();
        assert_eq!(suffix.len(), count * 2);
        assert!(suffix.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
    }
}

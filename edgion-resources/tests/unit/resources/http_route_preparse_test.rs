use super::*;

// =========================================================================
// Group J: HTTPRoute path matcher normalization tests
// =========================================================================

fn make_route_with_path(match_type: &str, value: &str) -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "test", "namespace": "default"},
        "spec": {
            "rules": [{
                "matches": [{
                    "path": {"type": match_type, "value": value}
                }]
            }]
        }
    }))
    .expect("route should deserialize")
}

fn get_path_value(route: &HTTPRoute) -> &str {
    route.spec.rules.as_ref().unwrap()[0].matches.as_ref().unwrap()[0]
        .path
        .as_ref()
        .unwrap()
        .value
        .as_deref()
        .unwrap()
}

#[test]
fn http_route_path_match_exact_normalized() {
    // Trailing slash should be removed by dot-segment/normalization logic.
    // normalize_config_path("/admin/") should produce "/admin/" — actually trailing
    // slash is not removed by the BASE algorithm; test with a genuine normalizable input.
    // Input with percent-encoded slash: "/api%2Fv1" → "/api/v1"
    let mut route = make_route_with_path("Exact", "/admin/");
    route.normalize_path_matchers();
    // "/admin/" has no dot-segments or percent-encoding; normalize is a no-op → unchanged
    assert_eq!(get_path_value(&route), "/admin/");

    // A value that genuinely needs normalization (double slash collapsed)
    let mut route2 = make_route_with_path("Exact", "/admin//foo");
    route2.normalize_path_matchers();
    assert_eq!(get_path_value(&route2), "/admin/foo");
}

#[test]
fn http_route_path_match_exact_percent_encoded_normalized() {
    // Percent-encoded slash should be decoded: "/api%2Fv1" → "/api/v1"
    let mut route = make_route_with_path("Exact", "/api%2Fv1");
    route.normalize_path_matchers();
    assert_eq!(get_path_value(&route), "/api/v1");
}

#[test]
fn http_route_path_match_pathprefix_normalized() {
    // Percent-encoded slash decoded: "/api%2Fv1" → "/api/v1"
    let mut route = make_route_with_path("PathPrefix", "/api%2Fv1");
    route.normalize_path_matchers();
    assert_eq!(get_path_value(&route), "/api/v1");
}

#[test]
fn prefix_trailing_slash_is_canonical_in_declared_and_resolved_rules() {
    for (kind, input, expected) in [
        ("PathPrefix", "/abc/", "/abc"),
        ("PathPrefix", "/abc///", "/abc"),
        ("PathPrefix", "/abc%2F", "/abc"),
        ("PathPrefix", "/abc/./", "/abc"),
        ("PathPrefix", "/users/:id/", "/users/:id"),
        ("PathPrefix", "/", "/"),
        ("PathPrefix", "///", "/"),
        ("Exact", "/abc/", "/abc/"),
        ("RegularExpression", "/abc/", "/abc/"),
    ] {
        let mut route = make_route_with_path(kind, input);
        route.spec.resolved_rules = route.spec.rules.clone();
        for _ in 0..2 {
            route.normalize_path_matchers();
            assert_eq!(get_path_value(&route), expected, "{kind} {input}");
            let resolved = route.spec.resolved_rules.as_ref().unwrap()[0].matches.as_ref().unwrap()[0]
                .path
                .as_ref()
                .unwrap()
                .value
                .as_deref()
                .unwrap();
            assert_eq!(resolved, expected, "resolved {kind} {input}");
        }
    }
}

#[test]
fn http_route_path_match_regex_not_normalized() {
    // RegularExpression values must not be modified
    let input = "/admin%2F.*";
    let mut route = make_route_with_path("RegularExpression", input);
    route.normalize_path_matchers();
    assert_eq!(get_path_value(&route), input);
}

#[test]
fn http_route_path_match_already_normalized_unchanged() {
    // Already-normalized value — normalize_config_path is a no-op
    let mut route = make_route_with_path("Exact", "/api/v1");
    route.normalize_path_matchers();
    assert_eq!(get_path_value(&route), "/api/v1");
}

#[test]
fn http_route_path_match_dot_segment_normalized() {
    // Dot segments should be resolved: "/api/v1/./foo" → "/api/v1/foo"
    let mut route = make_route_with_path("PathPrefix", "/api/v1/./foo");
    route.normalize_path_matchers();
    assert_eq!(get_path_value(&route), "/api/v1/foo");
}

// =========================================================================
// resolved_rules preparse coverage (delegation)
// =========================================================================

#[test]
fn resolved_rules_covered_by_preparse_pipeline() {
    // A route whose delegation flatten produced resolved_rules must get the
    // same load-time processing as spec.rules: path normalization,
    // timeouts, retry, and annotation copies.
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {
            "name": "parent", "namespace": "infra",
            "annotations": {"edgion.io/forward-raw-path": "true"}
        },
        "spec": {
            "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/api"}}]}]
        }
    }))
    .unwrap();
    route.spec.resolved_rules = Some(
        serde_json::from_value(serde_json::json!([{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api//billing/./pay"}}],
            "timeouts": {"request": "10s"},
            "retry": {"attempts": 2}
        }]))
        .unwrap(),
    );

    route.preparse();
    route.parse_timeouts();
    route.parse_retry();

    let rule = &route.spec.resolved_rules.as_ref().unwrap()[0];
    assert_eq!(
        rule.matches.as_ref().unwrap()[0]
            .path
            .as_ref()
            .unwrap()
            .value
            .as_deref(),
        Some("/api/billing/pay"),
        "resolved_rules paths must be normalized"
    );
    assert!(rule.parsed_timeouts.is_some(), "resolved_rules timeouts must be parsed");
    assert_eq!(
        rule.parsed_retry.as_ref().and_then(|r| r.attempts),
        Some(2),
        "resolved_rules retry must be parsed"
    );
    assert_eq!(
        rule.parsed_forward_raw_path,
        Some(true),
        "annotation copies must reach resolved_rules"
    );
}

#[test]
fn http_route_retry_accepts_the_i32_domain_ceiling() {
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "retry-max", "namespace": "default"},
        "spec": {"rules": [{"retry": {"attempts": i32::MAX}}]}
    }))
    .unwrap();

    route.parse_retry();

    assert_eq!(
        route.spec.rules.as_ref().unwrap()[0]
            .parsed_retry
            .as_ref()
            .and_then(|retry| retry.attempts),
        Some(i32::MAX as u32)
    );
}

// =========================================================================
// forward-raw-path annotation parsing tests
// =========================================================================

fn make_test_http_route() -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "test-route", "namespace": "default"},
        "spec": {
            "rules": [
                {
                    "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}]
                }
            ]
        }
    }))
    .expect("test HTTPRoute should deserialize")
}

#[test]
fn parses_forward_raw_path_true() {
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
        "true".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_forward_raw_path, Some(true));
    }
}

#[test]
fn parses_forward_raw_path_false() {
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
        "false".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_forward_raw_path, Some(false));
    }
}

#[test]
fn parses_retry_non_idempotent_true() {
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::RETRY_NON_IDEMPOTENT.to_string(),
        "true".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_allow_non_idempotent_retry, Some(true));
    }
}

#[test]
fn parses_retry_non_idempotent_absent_is_none() {
    let mut route = make_test_http_route();
    route.metadata.annotations = None;
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_allow_non_idempotent_retry, None);
    }
}

#[test]
fn parses_retry_non_idempotent_invalid_is_none() {
    // Only lowercase "true"/"false" are recognized; anything else falls
    // back to the safe default (None → not opted in).
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::RETRY_NON_IDEMPOTENT.to_string(),
        "yes".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_allow_non_idempotent_retry, None);
    }
}

#[test]
fn parses_forward_raw_path_absent_is_none() {
    let mut route = make_test_http_route();
    route.metadata.annotations = None;
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_forward_raw_path, None);
    }
}

#[test]
fn parses_forward_raw_path_invalid_is_none() {
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
        "yes".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_forward_raw_path, None);
    }
}

#[test]
fn parses_forward_raw_path_uppercase_is_invalid() {
    // Strict match: only lowercase "true" / "false" accepted.
    // "True" / "TRUE" / "False" all log INFO and treat as None.
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
        "True".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_forward_raw_path, None);
    }
}

#[test]
fn parses_forward_raw_path_with_whitespace_is_invalid() {
    // Whitespace not trimmed: "  true  " is invalid.
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
        "  true  ".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_forward_raw_path, None);
    }
}

#[test]
fn parses_forward_raw_path_empty_is_invalid() {
    // Empty value: "" is invalid (operator likely meant to delete the annotation).
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
        String::new(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert_eq!(rule.parsed_forward_raw_path, None);
    }
}

// =========================================================================
// mirror-* tuning annotation parsing tests
// =========================================================================

#[test]
fn parses_mirror_tuning_absent_is_none() {
    let mut route = make_test_http_route();
    route.metadata.annotations = None;
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert!(rule.parsed_mirror_tuning.is_none());
    }
}

#[test]
fn parses_mirror_tuning_unrelated_annotations_is_none() {
    // Annotations present but none of the mirror-* keys → None (defaults).
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert("edgion.io/forward-raw-path".to_string(), "true".to_string());
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        assert!(rule.parsed_mirror_tuning.is_none());
    }
}

#[test]
fn parses_mirror_tuning_partial_keys_fill_defaults() {
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::MIRROR_CONNECT_TIMEOUT_MS.to_string(),
        "500".to_string(),
    );
    anns.insert(
        crate::constants::annotations::edgion::MIRROR_LOG.to_string(),
        "false".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    for rule in route.spec.rules.as_ref().unwrap() {
        let tuning = rule.parsed_mirror_tuning.as_ref().expect("tuning should be parsed");
        assert_eq!(tuning.connect_timeout_ms, 500);
        assert!(!tuning.mirror_log);
        // Unset keys fall back to defaults.
        let defaults = MirrorTuning::default();
        assert_eq!(tuning.write_timeout_ms, defaults.write_timeout_ms);
        assert_eq!(tuning.max_buffered_chunks, defaults.max_buffered_chunks);
        assert_eq!(tuning.max_concurrent, defaults.max_concurrent);
        assert_eq!(tuning.channel_full_timeout_ms, defaults.channel_full_timeout_ms);
    }
}

#[test]
fn parses_mirror_tuning_all_keys() {
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    let e = |k: &str| k.to_string();
    anns.insert(
        e(crate::constants::annotations::edgion::MIRROR_CONNECT_TIMEOUT_MS),
        "200".to_string(),
    );
    anns.insert(
        e(crate::constants::annotations::edgion::MIRROR_WRITE_TIMEOUT_MS),
        "300".to_string(),
    );
    anns.insert(
        e(crate::constants::annotations::edgion::MIRROR_CHANNEL_FULL_TIMEOUT_MS),
        "150".to_string(),
    );
    anns.insert(
        e(crate::constants::annotations::edgion::MIRROR_MAX_BUFFERED_CHUNKS),
        "8".to_string(),
    );
    anns.insert(
        e(crate::constants::annotations::edgion::MIRROR_MAX_BUFFERED_BYTES),
        "2MiB".to_string(),
    );
    anns.insert(
        e(crate::constants::annotations::edgion::MIRROR_LOG),
        "false".to_string(),
    );
    anns.insert(
        e(crate::constants::annotations::edgion::MIRROR_MAX_CONCURRENT),
        "64".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    let tuning = route.spec.rules.as_ref().unwrap()[0]
        .parsed_mirror_tuning
        .as_ref()
        .expect("tuning should be parsed");
    assert_eq!(tuning.connect_timeout_ms, 200);
    assert_eq!(tuning.write_timeout_ms, 300);
    assert_eq!(tuning.channel_full_timeout_ms, 150);
    assert_eq!(tuning.max_buffered_chunks, 8);
    assert_eq!(tuning.max_buffered_bytes, "2MiB");
    assert!(!tuning.mirror_log);
    assert_eq!(tuning.max_concurrent, 64);
}

#[test]
fn parses_mirror_tuning_invalid_value_falls_back_to_default() {
    // An invalid numeric/bool value is logged and replaced by the field
    // default; the other valid keys still apply.
    let mut route = make_test_http_route();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert(
        crate::constants::annotations::edgion::MIRROR_CONNECT_TIMEOUT_MS.to_string(),
        "not-a-number".to_string(),
    );
    anns.insert(
        crate::constants::annotations::edgion::MIRROR_MAX_CONCURRENT.to_string(),
        "32".to_string(),
    );
    route.metadata.annotations = Some(anns);
    route.preparse();
    let tuning = route.spec.rules.as_ref().unwrap()[0]
        .parsed_mirror_tuning
        .as_ref()
        .expect("tuning should be parsed");
    assert_eq!(tuning.connect_timeout_ms, MirrorTuning::default().connect_timeout_ms);
    assert_eq!(tuning.max_concurrent, 32);
}

#[test]
fn parses_mirror_tuning_capacity_fallback_preserves_valid_fields() {
    use crate::constants::annotations::edgion;
    for (key, values) in [
        (
            edgion::MIRROR_MAX_CONCURRENT,
            vec![
                "0".to_string(),
                "65537".to_string(),
                usize::MAX.to_string(),
                "184467440737095516160".to_string(),
            ],
        ),
        (
            edgion::MIRROR_MAX_BUFFERED_CHUNKS,
            vec!["0".to_string(), "1025".to_string(), usize::MAX.to_string()],
        ),
        (
            edgion::MIRROR_MAX_BUFFERED_BYTES,
            vec!["0".to_string(), "2GiB".to_string(), "bad-size".to_string()],
        ),
    ] {
        for value in values {
            let mut route = make_test_http_route();
            route.metadata.annotations = Some(std::collections::BTreeMap::from([
                (key.to_string(), value),
                (edgion::MIRROR_WRITE_TIMEOUT_MS.to_string(), "123".to_string()),
            ]));
            route.preparse();
            let tuning = route.spec.rules.as_ref().unwrap()[0]
                .parsed_mirror_tuning
                .as_ref()
                .unwrap();
            assert_eq!(
                tuning.resolve_limits().unwrap(),
                MirrorTuning::default().resolve_limits().unwrap()
            );
            assert_eq!(tuning.write_timeout_ms, 123);
        }
    }
}

#[test]
fn mirror_tuning_reaches_resolved_rules() {
    // Delegation-flattened rules must receive the same annotation copies.
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {
            "name": "parent", "namespace": "infra",
            "annotations": {"edgion.io/mirror-max-concurrent": "16"}
        },
        "spec": {
            "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/api"}}]}]
        }
    }))
    .unwrap();
    route.spec.resolved_rules = Some(
        serde_json::from_value(serde_json::json!([{
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/child"}}]
        }]))
        .unwrap(),
    );
    route.preparse();
    let rule = &route.spec.resolved_rules.as_ref().unwrap()[0];
    assert_eq!(
        rule.parsed_mirror_tuning.as_ref().map(|t| t.max_concurrent),
        Some(16),
        "annotation copies must reach resolved_rules"
    );
}

// =========================================================================
// retry.codes range validation (400..=599, matching the Gateway API CRD)
// =========================================================================

fn make_route_with_retry_codes(codes: Vec<i32>) -> HTTPRoute {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {"name": "test-route", "namespace": "default"},
        "spec": {
            "rules": [{
                "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                "retry": {"codes": codes}
            }]
        }
    }))
    .expect("retry route should deserialize")
}

#[test]
fn parse_retry_codes_clamped_to_400_599() {
    // 1xx/2xx/3xx codes are outside the Gateway API HTTPRouteRetry range and
    // must be dropped; only 400..=599 survive preparse.
    let mut route = make_route_with_retry_codes(vec![100, 103, 204, 302, 400, 503, 599, 600]);
    route.parse_retry();
    let parsed = route.spec.rules.as_ref().unwrap()[0]
        .parsed_retry
        .as_ref()
        .expect("parsed_retry should be set");
    assert_eq!(parsed.codes.as_deref(), Some([400u16, 503, 599].as_slice()));
}

#[test]
fn parse_retry_codes_all_out_of_range_yields_empty() {
    let mut route = make_route_with_retry_codes(vec![100, 200, 399]);
    route.parse_retry();
    let parsed = route.spec.rules.as_ref().unwrap()[0]
        .parsed_retry
        .as_ref()
        .expect("parsed_retry should be set");
    assert_eq!(parsed.codes.as_deref(), Some([].as_slice()));
}

#[test]
fn parse_retry_codes_deduplicate_in_first_seen_order_and_stay_bounded() {
    let mut codes = vec![503, 400, 503, 599, 400, 418, 418];
    for _ in 0..4 {
        codes.extend((400..=599).rev());
    }
    let mut expected = vec![503u16, 400, 599, 418];
    expected.extend(
        (400u16..=599)
            .rev()
            .filter(|code| !matches!(*code, 400 | 418 | 503 | 599)),
    );

    let mut route = make_route_with_retry_codes(codes);
    route.parse_retry();
    let parsed = route.spec.rules.as_ref().unwrap()[0]
        .parsed_retry
        .as_ref()
        .expect("parsed_retry should be set")
        .codes
        .as_ref()
        .expect("explicit retry.codes should remain present");

    assert_eq!(parsed, &expected, "valid codes must keep first-seen order");
    assert_eq!(
        parsed.len(),
        200,
        "the runtime vector cannot exceed the valid code domain"
    );
}

use super::*;

#[test]
fn malformed_backend_filter_remains_backend_scoped() {
    let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "backendRefs": [
            {
                "name": "invalid",
                "port": 8080,
                "filters": [{"type": "ExtensionRef"}]
            },
            {"name": "valid", "port": 8080}
        ]
    }))
    .unwrap();

    let runtime = HttpRouteRuleUnit::compile_http_rule(&rule, "route-ns").unwrap();

    assert!(runtime.backends[0].plugin_runtime.rejects_invalid_config());
    assert!(!runtime.backends[1].plugin_runtime.rejects_invalid_config());
    assert!(!runtime.plugin_runtime.rejects_invalid_config());
}

#[test]
fn compile_http_rule_preserves_slots_and_types_backends() {
    let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "backendRefs": [
            {"name": "service-a", "port": 8080, "weight": 7},
            {"kind": "ServiceExternalName", "name": "dns-svc", "port": 30001, "weight": 1},
            {"kind": "ServiceImport", "name": "imported-svc", "port": 8080, "weight": 2},
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "ai-a", "weight": 3},
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "ai-bad", "port": 443},
            {"group": "example.io", "kind": "Service", "name": "wrong-group-service"}
        ]
    }))
    .unwrap();

    let runtime = HttpRouteRuleUnit::compile_http_rule(&rule, "route-ns").unwrap();
    assert_eq!(runtime.backends.len(), 6);
    assert_eq!(
        runtime.backends.iter().map(|backend| backend.slot).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5]
    );
    assert_eq!(runtime.backends[0].schema.weight, Some(7));
    assert_eq!(
        runtime.backends[0].target,
        CompiledBackendTarget::Service {
            namespace: "route-ns".to_string(),
            name: "service-a".to_string()
        }
    );
    assert_eq!(
        runtime.backends[1].target,
        CompiledBackendTarget::Invalid {
            classified_kind: HTTPBackendRefKind::Unsupported
        }
    );
    assert_eq!(
        runtime.backends[2].target,
        CompiledBackendTarget::Invalid {
            classified_kind: HTTPBackendRefKind::Unsupported
        }
    );
    assert_eq!(
        runtime.backends[3].target,
        CompiledBackendTarget::EdgionBackend {
            namespace: "route-ns".to_string(),
            name: "ai-a".to_string()
        }
    );
    assert_eq!(
        runtime.backends[4].target,
        CompiledBackendTarget::Invalid {
            classified_kind: HTTPBackendRefKind::EdgionBackend
        }
    );
    assert_eq!(
        runtime.backends[5].target,
        CompiledBackendTarget::Invalid {
            classified_kind: HTTPBackendRefKind::Unsupported
        }
    );

    let selector = HttpRouteRuleUnit::build_backend_selector(&runtime.backends);
    // Service, EdgionBackend, and invalid slots retain their declared
    // traffic share. Two complete WRR cycles make the assertion independent
    // of the smooth sequence's partial-cycle ordering.
    let mut seen = [0u32; 6];
    for _ in 0..30 {
        let selected = selector.select().unwrap();
        seen[selected] += 1;
    }
    assert_eq!(seen, [14, 2, 4, 6, 2, 2]);
}

fn make_specificity(
    has_method: bool,
    header_count: u16,
    query_count: u16,
    creation_ts_ms: i64,
    route_key: &str,
    rule_index: u32,
    match_index: u32,
) -> MatchSpecificity {
    MatchSpecificity {
        has_method,
        header_count,
        query_count,
        creation_ts_ms,
        route_key: route_key.to_string(),
        rule_index,
        match_index,
    }
}

#[test]
fn test_specificity_method_wins() {
    let with_method = make_specificity(true, 0, 0, 1000, "ns/a", 0, 0);
    let without_method = make_specificity(false, 0, 0, 1000, "ns/a", 0, 0);
    assert_eq!(with_method.cmp_precedence(&without_method), Ordering::Less);
    assert_eq!(without_method.cmp_precedence(&with_method), Ordering::Greater);
}

#[test]
fn test_specificity_more_headers_wins() {
    let h3 = make_specificity(true, 3, 0, 1000, "ns/a", 0, 0);
    let h2 = make_specificity(true, 2, 0, 1000, "ns/a", 0, 0);
    let h0 = make_specificity(true, 0, 0, 1000, "ns/a", 0, 0);
    assert_eq!(h3.cmp_precedence(&h2), Ordering::Less);
    assert_eq!(h2.cmp_precedence(&h0), Ordering::Less);
    assert_eq!(h3.cmp_precedence(&h0), Ordering::Less);
}

#[test]
fn test_specificity_more_query_params_wins() {
    let q2 = make_specificity(true, 1, 2, 1000, "ns/a", 0, 0);
    let q1 = make_specificity(true, 1, 1, 1000, "ns/a", 0, 0);
    let q0 = make_specificity(true, 1, 0, 1000, "ns/a", 0, 0);
    assert_eq!(q2.cmp_precedence(&q1), Ordering::Less);
    assert_eq!(q1.cmp_precedence(&q0), Ordering::Less);
}

#[test]
fn test_specificity_older_timestamp_wins() {
    let old = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
    let new = make_specificity(true, 1, 1, 200, "ns/a", 0, 0);
    assert_eq!(old.cmp_precedence(&new), Ordering::Less);
    assert_eq!(new.cmp_precedence(&old), Ordering::Greater);
}

#[test]
fn test_specificity_alphabetical_route_key() {
    let a = make_specificity(true, 1, 1, 100, "ns/aaa", 0, 0);
    let b = make_specificity(true, 1, 1, 100, "ns/bbb", 0, 0);
    assert_eq!(a.cmp_precedence(&b), Ordering::Less);
    assert_eq!(b.cmp_precedence(&a), Ordering::Greater);
}

#[test]
fn test_specificity_rule_index_tiebreak() {
    let r0 = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
    let r1 = make_specificity(true, 1, 1, 100, "ns/a", 1, 0);
    assert_eq!(r0.cmp_precedence(&r1), Ordering::Less);
}

#[test]
fn test_specificity_match_index_tiebreak() {
    let m0 = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
    let m1 = make_specificity(true, 1, 1, 100, "ns/a", 0, 1);
    assert_eq!(m0.cmp_precedence(&m1), Ordering::Less);
}

#[test]
fn test_specificity_equal() {
    let a = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
    let b = make_specificity(true, 1, 1, 100, "ns/a", 0, 0);
    assert_eq!(a.cmp_precedence(&b), Ordering::Equal);
}

#[test]
fn test_specificity_method_trumps_headers() {
    let with_method = make_specificity(true, 0, 0, 1000, "ns/a", 0, 0);
    let many_headers = make_specificity(false, 10, 10, 100, "ns/a", 0, 0);
    assert_eq!(with_method.cmp_precedence(&many_headers), Ordering::Less);
}

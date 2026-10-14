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

    let runtime = crate::routes::http::match_unit::compile_http_fixture(&rule, "route-ns").unwrap();

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

    let runtime = crate::routes::http::match_unit::compile_http_fixture(&rule, "route-ns").unwrap();
    assert_eq!(runtime.backends.len(), 6);
    assert_eq!(
        runtime.backends.iter().map(|backend| backend.slot).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5]
    );
    assert_eq!(runtime.backends[0].schema.weight, Some(7));
    assert!(
        matches!(&runtime.backends[0].target, CompiledBackendTarget::Service(target)
        if target.namespace() == "route-ns" && target.name() == "service-a")
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
    assert!(
        matches!(&runtime.backends[3].target, CompiledBackendTarget::EdgionBackend(target)
        if target.namespace() == "route-ns" && target.name() == "ai-a")
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

#[test]
fn actual_owner_gates_factories_and_effective_backend_proofs() {
    use serde_json::json;
    let baseline =
        json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":[{"name":"api","port":80,"weight":7}]}]}});
    for (field, value) in [
        ("name", json!(null)),
        ("name", json!("")),
        ("name", json!("bad/name")),
        ("namespace", json!("")),
        ("namespace", json!("UPPER")),
    ] {
        let mut input = baseline.clone();
        input["metadata"][field] = value;
        let route: HTTPRoute = serde_json::from_value(input).unwrap();
        let mut rule_reads = 0;
        let mut backend_reads = 0;
        assert!(HttpRouteRuleUnit::compile_with_factories(
            &route,
            0,
            |_, _| {
                rule_reads += 1;
                Arc::new(PluginRuntime::new())
            },
            |_, _, _| {
                backend_reads += 1;
                Arc::new(PluginRuntime::new())
            }
        )
        .is_none());
        assert_eq!((rule_reads, backend_reads), (0, 0));
    }
    for (field, value) in [
        ("group", json!("core")),
        ("group", json!("example.io")),
        ("kind", json!("ServiceImport")),
        ("kind", json!("ServiceExternalName")),
        ("kind", json!("service")),
        ("name", json!("bad/name")),
        ("namespace", json!("")),
        ("port", json!(null)),
        ("port", json!(0)),
        ("port", json!(-1)),
        ("port", json!(65536)),
        ("refDenied", json!({"targetNamespace":"default","targetName":"api"})),
    ] {
        let mut input = baseline.clone();
        input["spec"]["rules"][0]["backendRefs"][0][field] = value;
        let route: HTTPRoute = serde_json::from_value(input).unwrap();
        let mut rule_reads = 0;
        let mut backend_reads = 0;
        let compiled = HttpRouteRuleUnit::compile_with_factories(
            &route,
            0,
            |_, _| {
                rule_reads += 1;
                Arc::new(PluginRuntime::new())
            },
            |_, _, _| {
                backend_reads += 1;
                Arc::new(PluginRuntime::new())
            },
        )
        .unwrap();
        assert_eq!((rule_reads, backend_reads), (1, 0), "{field}");
        assert_eq!(compiled.backends.len(), 1);
        assert_eq!(compiled.backends[0].slot(), 0);
        assert_eq!(compiled.backends[0].weight, Some(7));
        assert!(compiled.backends[0].permitted_service().is_none());
        assert_eq!(
            HttpRouteRuleUnit::build_backend_selector(&compiled.backends)
                .select()
                .unwrap(),
            0
        );
    }
    let mut route: HTTPRoute = serde_json::from_value(baseline).unwrap();
    let mut calls = 0;
    let authored = HttpRouteRuleUnit::compile_with_factories(
        &route,
        0,
        |owner, _| {
            assert_eq!(owner.metadata.namespace.as_deref().unwrap_or("default"), "default");
            Arc::new(PluginRuntime::new())
        },
        |owner, _, _| {
            assert_eq!(owner.metadata.namespace.as_deref().unwrap_or("default"), "default");
            calls += 1;
            Arc::new(PluginRuntime::new())
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(authored.backends[0].permitted_service().unwrap().key(), "default/api");
    route.spec.resolved_rules = Some(vec![serde_json::from_value(json!({"backendRefs":[
        {"name":"api","namespace":"child","port":1,"weight":0},
        {"name":"api","namespace":"child","port":65535,"weight":3},
        {"name":"denied","namespace":"child","port":80,"weight":5,"refDenied":{"targetNamespace":"child","targetName":"denied"}}
    ]})).unwrap()]);
    calls = 0;
    let effective = HttpRouteRuleUnit::compile_with_factories(
        &route,
        0,
        |_, _| Arc::new(PluginRuntime::new()),
        |_, _, _| {
            calls += 1;
            Arc::new(PluginRuntime::new())
        },
    )
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(
        effective
            .backends
            .iter()
            .map(|b| (b.slot(), b.weight))
            .collect::<Vec<_>>(),
        vec![(0, Some(0)), (1, Some(3)), (2, Some(5))]
    );
    assert_eq!(effective.backends[0].permitted_service().unwrap().key(), "child/api");
    assert_ne!(
        effective.backends[0].permitted_service().unwrap().section(),
        effective.backends[1].permitted_service().unwrap().section()
    );
    assert!(effective.backends[2].permitted_service().is_none());
    assert_eq!(authored.backends[0].permitted_service().unwrap().key(), "default/api");
}

#[test]
fn session_lifetime_direct_compile_uses_effective_rule_without_preparse() {
    for resolved in [false, true] {
        for (policy, expected, valid) in [
            (serde_json::json!(null), None, true),
            (serde_json::json!({}), None, true),
            (serde_json::json!({"absoluteTimeout": "1500ms"}), Some(2), true),
            (serde_json::json!({"absoluteTimeout": "0s"}), Some(1), true),
            (serde_json::json!({"absoluteTimeout": "bad"}), None, false),
            (serde_json::json!({"absoluteTimeout": " 1s"}), None, false),
            (
                serde_json::json!({"cookieConfig": {"lifetimeType": "Permanent"}}),
                None,
                false,
            ),
        ] {
            let mut route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
                "metadata": {"name": "session-compile", "namespace": "default"},
                "spec": {"rules": [{"sessionPersistence": {"absoluteTimeout": "99s"}}]}
            }))
            .unwrap();
            let rule = serde_json::from_value(serde_json::json!({"sessionPersistence": policy})).unwrap();
            if resolved {
                route.spec.resolved_rules = Some(vec![rule]);
            } else {
                route.spec.rules = Some(vec![rule]);
            }
            // conf_sync transports only policy text; compilation derives the scalar anew.
            let wire = serde_json::to_string(&route).unwrap();
            assert!(!wire.contains("session_persistence_ttl_secs"));
            assert!(!wire.contains("sessionPersistenceTtlSecs"));
            let route = serde_json::from_str(&wire).unwrap();
            let runtime = HttpRouteRuleUnit::compile_http_rule(&route, 0);
            if valid {
                assert_eq!(runtime.unwrap().session_persistence_ttl_secs, expected);
            } else {
                assert!(runtime.is_err());
            }
        }
    }
}

#[test]
fn session_lifetime_invalid_policy_rejects_before_plugin_factories() {
    let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name":"invalid-session", "namespace":"default"},
        "spec": {"rules":[{"sessionPersistence":{"absoluteTimeout":"bad"}, "backendRefs":[{"name":"svc", "port":80}]}]}
    }))
    .unwrap();
    let runtime = HttpRouteRuleUnit::compile_with_factories(
        &route,
        0,
        |_, _| panic!("invalid session must reject before rule plugins"),
        |_, _, _| panic!("invalid session must reject before backend plugins"),
    );
    assert!(runtime.is_none());
}

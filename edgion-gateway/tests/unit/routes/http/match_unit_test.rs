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
    route.spec.resolved_rules = Some(vec![serde_json::from_value(json!({"resolvedParentMatch": {"kind": "direct"}, "backendRefs":[
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
                route.spec.resolved_rules.as_mut().unwrap()[0].resolved_parent_match =
                    Some(ResolvedParentMatch::Direct {});
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

#[test]
fn arch156_parent_paths_use_canonical_radix_conjunction() {
    for (prefixes, yes, no) in [
        (vec!["/api"], vec!["/api", "/api/secret"], vec!["/apix", "/apisecret"]),
        (vec!["/api/"], vec!["/api", "/api/secret"], vec!["/apix"]),
        (vec!["/"], vec!["/", "/anything"], vec!["relative"]),
        (
            vec!["/api/:version", "/api/:version/users"],
            vec!["/api/v1/users", "/api/v2/users/x"],
            vec!["/api/v1/other", "/api//users"],
        ),
        (
            vec!["/api/::literal"],
            vec!["/api/:literal", "/api/:literal/x"],
            vec!["/api/anything", "/api/::literal"],
        ),
        (vec!["/api"; 8], vec!["/api/x"], vec!["/apix"]),
    ] {
        let scope = ParentMatch {
            raw_path_prefixes: prefixes.iter().map(|p| p.to_string()).collect(),
        };
        let compiled = CompiledParentMatch::compile(&scope, &PathNormalizationConfig::default())
            .unwrap()
            .0;
        for path in yes {
            assert!(compiled.matches(path), "{prefixes:?} must match {path}");
        }
        for path in no {
            assert!(!compiled.matches(path), "{prefixes:?} must reject {path}");
        }
    }
}

#[test]
fn arch156_parent_match_provenance_fails_closed_before_factories() {
    let base = serde_json::json!({
        "metadata": {"name": "parent", "namespace": "default"},
        "spec": {"resolvedRules": [{"matches": [{"path": {"type": "RegularExpression", "value": "^/api"}}]}]}
    });
    for metadata in [
        serde_json::Value::Null,
        serde_json::json!({"kind": "delegated", "matches": []}),
        serde_json::json!({"kind": "delegated", "matches": [{"rawPathPrefixes": []}]}),
        serde_json::json!({"kind": "delegated", "matches": [{"rawPathPrefixes": ["relative"]}]}),
        serde_json::json!({"kind": "delegated", "matches": [{"rawPathPrefixes": ["/api"]}, {"rawPathPrefixes": ["/api"]}]}),
        serde_json::json!({"kind": "delegated", "matches": [{"rawPathPrefixes": vec!["/api"; 9]}]}),
    ] {
        let mut value = base.clone();
        value["spec"]["resolvedRules"][0]["resolvedParentMatch"] = metadata;
        let route: HTTPRoute = serde_json::from_value(value).unwrap();
        assert!(HttpRouteRuleUnit::compile_with_factories(
            &route,
            0,
            |_, _| panic!("invalid scope must be rejected before rule plugins"),
            |_, _, _| panic!("invalid scope must be rejected before backend plugins")
        )
        .is_none());
    }
    let mut value = base;
    value["spec"]["resolvedRules"][0]["resolvedParentMatch"] =
        serde_json::json!({"kind": "delegated", "matches": [{"rawPathPrefixes": ["/api"]}]});
    let route: HTTPRoute = serde_json::from_value(value).unwrap();
    let old = HttpRouteRuleUnit::compile_http_rule(&route, 0).unwrap();
    assert!(old.parent_matches[0].as_ref().unwrap().matches("/api/x"));
    let mut updated = route.clone();
    updated.spec.resolved_rules.as_mut().unwrap()[0].resolved_parent_match = Some(ResolvedParentMatch::Delegated {
        matches: vec![ParentMatch {
            raw_path_prefixes: vec!["/api/v1".into()],
        }],
    });
    let new = HttpRouteRuleUnit::compile_http_rule(&updated, 0).unwrap();
    assert!(!new.parent_matches[0].as_ref().unwrap().matches("/api/x"));
    assert!(old.parent_matches[0].as_ref().unwrap().matches("/api/x"));
    assert!(new.parent_matches[0].as_ref().unwrap().matches("/api/v1/x"));
}

#[test]
fn arch156_transported_parent_prefix_is_not_decoded_again() {
    use edgion_resources::path_normalization::{normalize_config_path, PathNormalizationConfig};
    use edgion_resources::ResourceMeta;

    for (authored, expected, escaped) in [
        ("/%%32%66", "/%2f", "/evil"),
        ("/%%34%31", "/%41", "/A/x"),
        ("/api/./v1/../", "/api/", "/elsewhere"),
    ] {
        // Transport authored paths; Gateway owns normalization under its policy.
        // Controller tests separately assert actual flattening emits these values.
        assert_eq!(
            normalize_config_path(authored, &PathNormalizationConfig::default()),
            expected
        );
        let prefix = authored;
        let outgoing: HTTPRoute = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "parent", "namespace": "default"},
            "spec": {"resolvedRules": [{
                "matches": [{"path": {"type": "RegularExpression", "value": "^/.*$"}}],
                "resolvedParentMatch": {"kind": "delegated", "matches": [{"rawPathPrefixes": [prefix]}]}
            }]}
        }))
        .unwrap();
        let wire = serde_json::to_vec(&outgoing).unwrap();
        let mut incoming: HTTPRoute = serde_json::from_slice(&wire).unwrap();
        incoming.pre_parse();
        let runtime = HttpRouteRuleUnit::compile_http_rule(&incoming, 0).unwrap();
        let guard = runtime.parent_matches[0].as_ref().unwrap();
        assert!(!guard.matches(escaped), "{authored} must not widen to accept {escaped}");
        assert!(guard.matches(expected), "{authored} must preserve {expected}");
        assert!(guard.matches(&format!("{}/x", expected.trim_end_matches('/'))));
    }
}

#[test]
fn arch156_literal_subset_elision_is_conservative() {
    for (kind, child, parents, elided) in [
        ("Exact", "/api", vec!["/api"], true),
        ("Exact", "/api/v1", vec!["/", "/api/"], true),
        ("PathPrefix", "/api/v1/", vec!["/api", "/api/v1"], true),
        ("PathPrefix", "/api/v1", vec!["/api"; 8], true),
        ("PathPrefix", "/apix", vec!["/api"], false),
        ("Exact", "/api", vec!["/api", "/api/v1"], false),
        ("RegularExpression", "^/api/v1$", vec!["/api"], false),
        ("PathPrefix", "/api/:id", vec!["/api"], false),
        ("Exact", "/api/v1", vec!["/api/:id"], false),
        ("PathPrefix", "/api/::literal", vec!["/api"], false),
        ("Exact", "/api/:literal", vec!["/api/::literal"], false),
    ] {
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "parent", "namespace": "default"},
            "spec": {"resolvedRules": [{
                "matches": [{"path": {"type": kind, "value": child}}],
                "resolvedParentMatch": {"kind": "delegated", "matches": [{"rawPathPrefixes": parents}]}
            }]}
        }))
        .unwrap();
        let guards = compile_parent_matches(
            &route,
            &route.effective_rules().unwrap()[0],
            &PathNormalizationConfig::default(),
        )
        .unwrap();
        assert_eq!(guards[0].is_none(), elided, "{kind} {child} under {parents:?}");
    }
}

#[test]
fn arch156_parent_guard_uses_gateway_normalization_policy() {
    for preserve in [false, true] {
        let config = PathNormalizationConfig {
            preserve_path_parameters: preserve,
            ..Default::default()
        };
        for raw in ["/api;tenant=admin", "/%%32%66", "/%%34%31", "/api/./v1/../"] {
            let scope = ParentMatch {
                raw_path_prefixes: vec![raw.into()],
            };
            let (guard, _) = CompiledParentMatch::compile(&scope, &config).unwrap();
            // Build an ordinary parent using the same canonical stages as
            // HTTPRoute preparse and the normal radix engine.
            let normalized = normalize_config_path(raw, &config);
            let path = RadixPath::new(&normalized, 0, true);
            let mut builder = PathRadixBuilder::new();
            builder.insert(path.tree_key(), 0).unwrap();
            let ordinary = builder.freeze().unwrap();
            for request in ["/api/evil", "/api;tenant=admin/x", "/evil", "/%2f/x", "/%41/x", "/A/x"] {
                let request = normalize_config_path(request, &config);
                let ordinary_match = ordinary
                    .match_all_ext(&request)
                    .iter()
                    .any(|(_, kind)| *kind != PathMatchKind::PartialSegment);
                assert_eq!(
                    guard.matches(&request),
                    ordinary_match,
                    "{raw} {request} preserve={preserve}"
                );
            }
            if raw == "/api;tenant=admin" {
                assert_eq!(guard.matches("/api/evil"), !preserve);
            }
        }
        // Elision uses the same actual policy, never Controller's lossy view.
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "parent", "namespace": "default"},
            "spec": {"resolvedRules": [{"matches": [{"path": {"value": "/api/evil"}}],
                "resolvedParentMatch": {"kind": "delegated", "matches": [{"rawPathPrefixes": ["/api;tenant=admin"]}]}
            }]}
        }))
        .unwrap();
        let guards = compile_parent_matches(&route, &route.effective_rules().unwrap()[0], &config).unwrap();
        assert_eq!(guards[0].is_some(), preserve);
    }
}

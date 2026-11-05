use super::*;
use serde_json::json;

fn pref(kind: Option<&str>, ns: Option<&str>, name: &str) -> ParentReference {
    ParentReference {
        group: None,
        kind: kind.map(str::to_string),
        namespace: ns.map(str::to_string),
        name: name.to_string(),
        section_name: None,
        port: None,
    }
}

#[test]
fn classify_gateway_only_is_root() {
    let refs = vec![pref(None, None, "gw"), pref(Some("Gateway"), None, "gw2")];
    assert_eq!(
        classify_parent_refs(Some(&refs), "ns", RouteKindName::HTTPRoute),
        ParentRefShape::Root
    );
}

#[test]
fn classify_no_refs_is_none() {
    assert_eq!(
        classify_parent_refs(None, "ns", RouteKindName::HTTPRoute),
        ParentRefShape::None
    );
    assert_eq!(
        classify_parent_refs(Some(&[]), "ns", RouteKindName::HTTPRoute),
        ParentRefShape::None
    );
}

#[test]
fn classify_one_to_three_route_refs_is_child() {
    let refs = vec![
        pref(Some("HTTPRoute"), None, "a"),
        pref(Some("HTTPRoute"), Some("other"), "b"),
        pref(Some("HTTPRoute"), None, "c"),
    ];
    assert_eq!(
        classify_parent_refs(Some(&refs), "ns", RouteKindName::HTTPRoute),
        ParentRefShape::Child(vec![
            ("ns".into(), "a".into()),
            ("other".into(), "b".into()),
            ("ns".into(), "c".into())
        ])
    );
}

#[test]
fn classify_four_route_refs_is_invalid() {
    let refs: Vec<_> = (0..4)
        .map(|i| pref(Some("HTTPRoute"), None, &format!("p{i}")))
        .collect();
    assert!(matches!(
        classify_parent_refs(Some(&refs), "ns", RouteKindName::HTTPRoute),
        ParentRefShape::Invalid(_)
    ));
}

#[test]
fn classify_mixed_gateway_and_route_is_invalid() {
    let refs = vec![pref(None, None, "gw"), pref(Some("HTTPRoute"), None, "p")];
    assert!(matches!(
        classify_parent_refs(Some(&refs), "ns", RouteKindName::HTTPRoute),
        ParentRefShape::Invalid(_)
    ));
}

#[test]
fn classify_grpc_ref_on_http_route_is_invalid() {
    let refs = vec![pref(Some("GRPCRoute"), None, "p")];
    assert!(matches!(
        classify_parent_refs(Some(&refs), "ns", RouteKindName::HTTPRoute),
        ParentRefShape::Invalid(_)
    ));
    assert!(matches!(
        classify_parent_refs(Some(&refs), "ns", RouteKindName::GRPCRoute),
        ParentRefShape::Child(_)
    ));
}

#[test]
fn classify_gateway_plus_service_kind_is_root() {
    let refs = vec![pref(None, None, "gw"), pref(Some("Service"), None, "svc")];
    assert_eq!(
        classify_parent_refs(Some(&refs), "ns", RouteKindName::HTTPRoute),
        ParentRefShape::Root
    );
}

fn http_rule(v: serde_json::Value) -> HTTPRouteRule {
    serde_json::from_value(v).expect("rule")
}

fn deleg_ref(name: &str) -> serde_json::Value {
    json!({"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": name})
}

#[test]
fn http_target_named_and_wildcard() {
    let r = http_rule(
        json!({"matches": [{"path": {"type": "PathPrefix", "value": "/a"}}], "backendRefs": [deleg_ref("child")]}),
    );
    let t = delegation_target_of_http_rule(&r, "ns").unwrap().unwrap();
    assert_eq!(t.ns, "ns");
    assert_eq!(t.name, DelegationTargetName::Named("child".into()));

    let mut b = deleg_ref("*");
    b["namespace"] = json!("other");
    let r = http_rule(json!({"backendRefs": [b]}));
    let t = delegation_target_of_http_rule(&r, "ns").unwrap().unwrap();
    assert_eq!(t.ns, "other");
    assert_eq!(t.name, DelegationTargetName::Wildcard);
}

#[test]
fn http_target_rejects_port() {
    let mut b = deleg_ref("c");
    b["port"] = json!(80);
    let r = http_rule(json!({"backendRefs": [b]}));
    assert!(delegation_target_of_http_rule(&r, "ns").unwrap().is_err());
}

#[test]
fn http_target_rejects_backend_filters() {
    let mut b = deleg_ref("c");
    b["filters"] = json!([{"type": "RequestHeaderModifier"}]);
    let r = http_rule(json!({"backendRefs": [b]}));
    assert!(delegation_target_of_http_rule(&r, "ns").unwrap().is_err());
}

#[test]
fn http_target_rejects_two_backend_refs() {
    let r = http_rule(json!({"backendRefs": [deleg_ref("a"), {"name": "svc"}]}));
    assert!(delegation_target_of_http_rule(&r, "ns").unwrap().is_err());
}

#[test]
fn http_target_rejects_non_prefix_match() {
    let r =
        http_rule(json!({"matches": [{"path": {"type": "Exact", "value": "/a"}}], "backendRefs": [deleg_ref("c")]}));
    assert!(delegation_target_of_http_rule(&r, "ns").unwrap().is_err());
}

#[test]
fn http_target_none_for_service_backend() {
    let r = http_rule(json!({"backendRefs": [{"name": "svc"}]}));
    assert!(delegation_target_of_http_rule(&r, "ns").is_none());
    assert!(delegation_target_of_http_rule(&http_rule(json!({})), "ns").is_none());
}

fn grpc_rule(v: serde_json::Value) -> GRPCRouteRule {
    serde_json::from_value(v).expect("rule")
}

fn grpc_ref(name: &str) -> serde_json::Value {
    json!({"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "name": name})
}

#[test]
fn grpc_target_requires_exact_service_without_method() {
    let ok = grpc_rule(json!({"matches": [{"method": {"service": "a.B"}}], "backendRefs": [grpc_ref("c")]}));
    let t = delegation_target_of_grpc_rule(&ok, "ns").unwrap().unwrap();
    assert_eq!(t.name, DelegationTargetName::Named("c".into()));

    for m in [
        json!({"method": {"type": "RegularExpression", "service": "a.B"}}),
        json!({"method": {"service": "a.B", "method": "M"}}),
        json!({"method": {"service": ""}}),
        json!({"method": {"method": "M"}}),
        json!({"headers": [{"name": "x", "value": "y"}]}),
    ] {
        let r = grpc_rule(json!({"matches": [m], "backendRefs": [grpc_ref("c")]}));
        assert!(delegation_target_of_grpc_rule(&r, "ns").unwrap().is_err());
    }
    assert!(delegation_target_of_grpc_rule(&grpc_rule(json!({"backendRefs": [{"name": "s"}]})), "ns").is_none());
}

#[test]
fn grpc_target_rejects_matchless() {
    let r = grpc_rule(json!({"backendRefs": [grpc_ref("c")]}));
    assert!(delegation_target_of_grpc_rule(&r, "ns").unwrap().is_err());
    let r = grpc_rule(json!({"matches": [], "backendRefs": [grpc_ref("c")]}));
    assert!(delegation_target_of_grpc_rule(&r, "ns").unwrap().is_err());
}

#[test]
fn filter_mask_ignores_extension_ref() {
    let r = http_rule(
        json!({"filters": [{"type": "ExtensionRef", "extensionRef": {"group": "g", "kind": "k", "name": "n"}}]}),
    );
    assert_eq!(standard_filter_mask_http(&r), 0);
    let g = grpc_rule(json!({"filters": [{"type": "ExtensionRef"}]}));
    assert_eq!(standard_filter_mask_grpc(&g), 0);
}

#[test]
fn filter_mask_distinct_bits_per_type() {
    let types = [
        "RequestHeaderModifier",
        "ResponseHeaderModifier",
        "RequestRedirect",
        "URLRewrite",
        "RequestMirror",
        "CORS",
        "ExternalAuth",
    ];
    let mut seen = 0u32;
    for t in types {
        let m = standard_filter_mask_http(&http_rule(json!({"filters": [{"type": t}]})));
        assert_eq!(m.count_ones(), 1, "{t}");
        assert_eq!(seen & m, 0, "{t} collides");
        seen |= m;
    }
    let both = standard_filter_mask_grpc(&grpc_rule(
        json!({"filters": [{"type": "RequestHeaderModifier"}, {"type": "ResponseHeaderModifier"}]}),
    ));
    assert_eq!(both.count_ones(), 2);
}

#[test]
fn http_filter_mask_includes_backend_ref_filters() {
    let rule_level = standard_filter_mask_http(&http_rule(json!({"filters": [{"type": "ResponseHeaderModifier"}]})));
    let backend_level = standard_filter_mask_http(&http_rule(json!({"backendRefs": [
        {"name": "svc", "port": 80, "filters": [{"type": "ResponseHeaderModifier"}]}
    ]})));
    assert_eq!(backend_level, rule_level, "backendRef filter must set the same bit");

    let combined = standard_filter_mask_http(&http_rule(json!({
        "filters": [{"type": "RequestHeaderModifier"}],
        "backendRefs": [
            {"name": "a", "port": 80, "filters": [{"type": "URLRewrite"}]},
            {"name": "b", "port": 80, "filters": [
                {"type": "ExtensionRef", "extensionRef": {"group": "g", "kind": "k", "name": "n"}}
            ]}
        ]
    })));
    let want = standard_filter_mask_http(&http_rule(
        json!({"filters": [{"type": "RequestHeaderModifier"}, {"type": "URLRewrite"}]}),
    ));
    assert_eq!(
        combined, want,
        "rule and backendRef bits are ORed; ExtensionRef adds none"
    );
}

#[test]
fn grpc_filter_mask_includes_backend_ref_filters() {
    let rule_level = standard_filter_mask_grpc(&grpc_rule(json!({"filters": [{"type": "ResponseHeaderModifier"}]})));
    let backend_level = standard_filter_mask_grpc(&grpc_rule(json!({"backendRefs": [
        {"name": "svc", "port": 50051, "filters": [{"type": "ResponseHeaderModifier"}]}
    ]})));
    assert_eq!(backend_level, rule_level, "backendRef filter must set the same bit");

    let combined = standard_filter_mask_grpc(&grpc_rule(json!({
        "filters": [{"type": "RequestHeaderModifier"}],
        "backendRefs": [
            {"name": "a", "port": 50051, "filters": [{"type": "ResponseHeaderModifier"}]},
            {"name": "b", "port": 50051, "filters": [{"type": "ExtensionRef"}]}
        ]
    })));
    assert_eq!(combined.count_ones(), 2);
}

#[test]
fn delegation_key_wildcard_serializes_as_star() {
    let key = key_for(
        RouteKindName::HTTPRoute,
        "p",
        "parent",
        &DelegationTarget {
            ns: "t".into(),
            name: DelegationTargetName::Wildcard,
        },
    );
    let v = serde_json::to_value(&key).unwrap();
    assert_eq!(v["target_name"], "*");
    let back: DelegationKey = serde_json::from_value(v).unwrap();
    assert_eq!(back, key);
}

#[test]
fn empty_keys_not_serialized() {
    let route: crate::resources::HTTPRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": "r", "namespace": "ns"}, "spec": {}
    }))
    .unwrap();
    let s = serde_json::to_string(&route.spec).unwrap();
    assert!(!s.contains("resolvedDelegationKeys"), "{s}");
    let g: crate::resources::GRPCRoute = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
        "metadata": {"name": "r", "namespace": "ns"}, "spec": {}
    }))
    .unwrap();
    assert!(!serde_json::to_string(&g.spec)
        .unwrap()
        .contains("resolvedDelegationKeys"));
}

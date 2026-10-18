use super::*;

use crate::runtime::store::listener_gateway_info::ListenerEntry;

#[test]
fn parent_port_keeps_shared_listener_ownership_with_the_declared_gateway() {
    for kind in ["HTTPRoute", "GRPCRoute", "TCPRoute", "UDPRoute"] {
        for section in [None, Some("shared")] {
            let mut a = resolved("default/a", "default", section);
            a.port = Some(8080);
            let mut b = resolved("default/b", "default", section);
            b.port = Some(8081);
            let entry = |name| {
                let mut entry = listener_entry("default", name, Some("shared"), None, None);
                entry.port = 8081;
                entry
            };
            let index = GatewayInfoIndex::from_entries_for_test(vec![
                ("default/a".into(), entry("a")),
                ("default/b".into(), entry("b")),
            ]);
            assert!(check_gateway_listener_match(&[a.clone()], &index, "", "default", kind).is_none());
            let owner = check_gateway_listener_match(&[a.clone(), b], &index, "", "default", kind).unwrap();
            assert_eq!(owner.gateway_name(), "b", "{kind}: {section:?}");
            a.port = None;
            assert_eq!(
                check_gateway_listener_match(&[a], &index, "", "default", kind)
                    .unwrap()
                    .gateway_name(),
                "a",
                "an omitted port must retain its existing semantics"
            );
        }
    }
}

fn resolved(gateway_key: &str, parent_ns: &str, section_name: Option<&str>) -> ResolvedParentRef {
    ResolvedParentRef {
        gateway_key: gateway_key.to_string(),
        parent_ns: parent_ns.to_string(),
        section_name: section_name.map(str::to_string),
        port: None,
    }
}

fn listener_entry(
    gateway_ns: &str,
    gateway_name: &str,
    listener_name: Option<&str>,
    hostname: Option<&str>,
    allowed_routes: Option<AllowedRoutes>,
) -> ListenerEntry {
    ListenerEntry {
        port: 8080,
        info: Arc::new(GatewayInfo::new(
            Some(gateway_ns.to_string()),
            gateway_name.to_string(),
            listener_name.map(str::to_string),
            None,
            None,
            None,
            Vec::new(),
            None,
        )),
        gateway_proof: None,
        hostname: hostname.map(str::to_string),
        allowed_routes,
    }
}

/// A cross-namespace parentRef against a listener with no `allowedRoutes`
/// must be rejected: `check_allowed_routes` falls back to
/// `route_namespace == gateway_namespace`.
///
/// This is the regression guard for the aliasing trap in this function.
/// `route_ns` (the ROUTE's namespace) and `ResolvedParentRef::parent_ns`
/// (the GATEWAY's) are equal in the same-namespace case, so reusing one for
/// both compiles and passes every same-namespace test while silently
/// admitting the cross-namespace route this test rejects.
#[test]
fn cross_namespace_parent_ref_is_rejected_without_allowed_routes() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), None, None),
    )]);
    let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", Some("http"))];

    // Route lives in `route-ns`, Gateway in `gw-ns` -> denied.
    assert!(
        check_gateway_listener_match(&parent_refs, &index, "example.com", "route-ns", "HTTPRoute").is_none(),
        "a cross-namespace route must not attach without allowedRoutes"
    );

    // Same reference, route now co-located with the Gateway -> allowed.
    assert!(
        check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute").is_some(),
        "a same-namespace route must still attach"
    );
}

/// The same guard on the no-sectionName path, which iterates listeners
/// instead of taking the `by_name` fast path.
#[test]
fn cross_namespace_parent_ref_is_rejected_on_the_no_section_name_path() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), None, None),
    )]);
    let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", None)];

    assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "route-ns", "HTTPRoute").is_none());
    assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute").is_some());
}

/// `from: "All"` is what actually authorizes a cross-namespace attachment,
/// and it must still be honoured after the refactor.
#[test]
fn cross_namespace_parent_ref_is_allowed_by_from_all() {
    use edgion_resources::resources::gateway::RouteNamespaces;

    let allowed = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("All".to_string()),
            selector: None,
        }),
        kinds: None,
    };
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), None, Some(allowed)),
    )]);
    let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", Some("http"))];

    assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "route-ns", "HTTPRoute").is_some());
}

/// The resolved key is the only thing consulted for the index lookup, so a
/// key naming no Gateway simply misses instead of matching something else.
#[test]
fn an_unknown_gateway_key_matches_nothing() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), None, None),
    )]);
    let parent_refs = vec![resolved("gw-ns/absent", "gw-ns", Some("http"))];

    assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute").is_none());
}

#[test]
fn missing_gateway_or_listener_continues_to_later_parent_refs() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), None, None),
    )]);
    let parent_refs = vec![
        resolved("gw-ns/absent", "gw-ns", Some("http")),
        resolved("gw-ns/gw", "gw-ns", Some("absent")),
        resolved("gw-ns/gw", "gw-ns", Some("http")),
    ];

    let matched = check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute")
        .expect("the final parent reference names an eligible listener");
    assert!(Arc::ptr_eq(matched, &index.get("gw-ns/gw", "http").unwrap().info));
}

#[test]
fn an_explicit_missing_section_does_not_fall_back_to_a_sibling() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), None, None),
    )]);
    let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", Some("absent"))];

    assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute").is_none());
}

#[test]
fn a_more_specific_listener_blocks_fallback_even_when_it_denies_the_route() {
    use edgion_resources::resources::gateway::RouteNamespaces;

    let allow_all = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("All".to_string()),
            selector: None,
        }),
        kinds: None,
    };
    let index = GatewayInfoIndex::from_entries_for_test(vec![
        (
            "gw-ns/gw".to_string(),
            listener_entry("gw-ns", "gw", Some("catch-all"), None, Some(allow_all)),
        ),
        (
            "gw-ns/gw".to_string(),
            listener_entry("gw-ns", "gw", Some("exact"), Some("example.com"), None),
        ),
    ]);

    for section_name in [Some("catch-all"), None] {
        let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", section_name)];
        assert!(
            check_gateway_listener_match(&parent_refs, &index, "example.com", "route-ns", "HTTPRoute").is_none(),
            "the exact listener's namespace denial must not enable catch-all fallback"
        );
        assert!(
            check_gateway_listener_match(&parent_refs, &index, "other.com", "route-ns", "HTTPRoute").is_some(),
            "the catch-all listener allows the route when no more-specific listener matches"
        );
    }
}

#[test]
fn named_and_unnamed_matches_share_the_index_gateway_info() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), None, None),
    )]);
    let expected = &index.get("gw-ns/gw", "http").unwrap().info;

    for section_name in [Some("http"), None] {
        let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", section_name)];
        let matched = check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute")
            .expect("the sole listener allows this same-namespace route");
        assert!(Arc::ptr_eq(matched, expected));
    }
}

#[test]
fn another_gateways_empty_exact_listener_blocks_broader_routes() {
    for broad in [None, Some("*.example.com")] {
        let index = GatewayInfoIndex::from_entries_for_test(vec![
            (
                "ns/broad".into(),
                listener_entry("ns", "broad", Some("broad"), broad, None),
            ),
            (
                "ns/exact".into(),
                listener_entry("ns", "exact", Some("exact"), Some("api.example.com"), None),
            ),
        ]);
        for section in [None, Some("broad")] {
            let parents = [resolved("ns/broad", "ns", section)];
            for kind in ["HTTPRoute", "GRPCRoute", "TLSRoute"] {
                assert!(check_gateway_listener_match(&parents, &index, "api.example.com", "ns", kind).is_none());
                assert!(check_gateway_listener_match(&parents, &index, "other.example.com", "ns", kind).is_some());
            }
        }
    }
}

#[test]
fn cross_gateway_wildcard_specificity_does_not_follow_route_parent_order() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![
        (
            "ns/broad".into(),
            listener_entry("ns", "broad", Some("broad"), Some("*.example.com"), None),
        ),
        (
            "ns/narrow".into(),
            listener_entry("ns", "narrow", Some("narrow"), Some("*.sub.example.com"), None),
        ),
    ]);
    for named in [false, true] {
        let parents = [
            resolved("ns/broad", "ns", named.then_some("broad")),
            resolved("ns/narrow", "ns", named.then_some("narrow")),
        ];
        let owner = check_gateway_listener_match(&parents, &index, "a.b.sub.example.com", "ns", "HTTPRoute").unwrap();
        assert_eq!(owner.gateway_name(), "narrow");
        let owner = check_gateway_listener_match(&parents, &index, "other.example.com", "ns", "HTTPRoute").unwrap();
        assert_eq!(owner.gateway_name(), "broad");
    }
}

/// parentRefs are tried in order and the first match wins.
#[test]
fn the_first_matching_parent_ref_wins() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![
        (
            "gw-ns/first".to_string(),
            listener_entry("gw-ns", "first", Some("http"), None, None),
        ),
        (
            "gw-ns/second".to_string(),
            listener_entry("gw-ns", "second", Some("http"), None, None),
        ),
    ]);
    let parent_refs = vec![
        resolved("gw-ns/first", "gw-ns", Some("http")),
        resolved("gw-ns/second", "gw-ns", Some("http")),
    ];

    let matched = check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute")
        .expect("one of the two gateways must match");
    assert_eq!(matched.gateway_name(), "first");
}

/// Listener hostname isolation is unaffected by the refactor.
#[test]
fn listener_hostname_isolation_still_applies() {
    let index = GatewayInfoIndex::from_entries_for_test(vec![(
        "gw-ns/gw".to_string(),
        listener_entry("gw-ns", "gw", Some("http"), Some("*.example.com"), None),
    )]);
    let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", Some("http"))];

    assert!(check_gateway_listener_match(&parent_refs, &index, "api.example.com", "gw-ns", "HTTPRoute").is_some());
    assert!(check_gateway_listener_match(&parent_refs, &index, "other.org", "gw-ns", "HTTPRoute").is_none());
}

#[test]
fn test_hostname_exact_match() {
    assert!(hostname_matches_listener("example.com", "example.com"));
    assert!(hostname_matches_listener("Example.COM", "example.com"));
    assert!(!hostname_matches_listener("other.com", "example.com"));
}

#[test]
fn test_hostname_wildcard_match() {
    // Single-level subdomain
    assert!(hostname_matches_listener("api.example.com", "*.example.com"));
    assert!(hostname_matches_listener("foo.example.com", "*.example.com"));

    // Multi-level subdomain (per Gateway API spec, wildcards match any depth)
    assert!(hostname_matches_listener("foo.api.example.com", "*.example.com"));
    assert!(hostname_matches_listener("a.b.c.example.com", "*.example.com"));

    // Wildcard should NOT match the domain itself
    assert!(!hostname_matches_listener("example.com", "*.example.com"));

    // Wildcard should NOT match different domain
    assert!(!hostname_matches_listener("api.other.com", "*.example.com"));
}

#[test]
fn test_allowed_routes_none_defaults_to_same() {
    // Per Gateway API spec, no AllowedRoutes defaults to Same namespace
    assert!(check_allowed_routes(&None, "default", "HTTPRoute", "default"));
    assert!(!check_allowed_routes(&None, "other", "HTTPRoute", "default"));
}

#[test]
fn test_allowed_routes_same_namespace() {
    use edgion_resources::resources::gateway::RouteNamespaces;

    let allowed = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("Same".to_string()),
            selector: None,
        }),
        kinds: None,
    };

    assert!(check_allowed_routes(
        &Some(allowed.clone()),
        "default",
        "HTTPRoute",
        "default"
    ));
    assert!(!check_allowed_routes(&Some(allowed), "other", "HTTPRoute", "default"));
}

#[test]
fn test_allowed_routes_all_namespace() {
    use edgion_resources::resources::gateway::RouteNamespaces;

    let allowed = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("All".to_string()),
            selector: None,
        }),
        kinds: None,
    };

    assert!(check_allowed_routes(
        &Some(allowed.clone()),
        "default",
        "HTTPRoute",
        "default"
    ));
    assert!(check_allowed_routes(&Some(allowed), "other", "HTTPRoute", "default"));
}

#[test]
fn test_allowed_routes_selector_trusts_controller() {
    use edgion_resources::resources::gateway::RouteNamespaces;

    let allowed = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("Selector".to_string()),
            selector: Some(serde_json::json!({ "matchLabels": { "env": "prod" } })),
        }),
        kinds: None,
    };

    // Selector is trusted (controller already validated), so both pass
    assert!(check_allowed_routes(
        &Some(allowed.clone()),
        "default",
        "HTTPRoute",
        "default"
    ));
    assert!(check_allowed_routes(&Some(allowed), "other", "HTTPRoute", "default"));
}

#[test]
fn test_allowed_routes_kind_filtering() {
    use edgion_resources::resources::gateway::{RouteGroupKind, RouteNamespaces};

    // Only HTTPRoute allowed
    let allowed = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("All".to_string()),
            selector: None,
        }),
        kinds: Some(vec![RouteGroupKind {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: "HTTPRoute".to_string(),
        }]),
    };

    assert!(check_allowed_routes(
        &Some(allowed.clone()),
        "default",
        "HTTPRoute",
        "default"
    ));
    assert!(!check_allowed_routes(
        &Some(allowed.clone()),
        "default",
        "GRPCRoute",
        "default"
    ));
    assert!(!check_allowed_routes(&Some(allowed), "default", "TLSRoute", "default"));
}

#[test]
fn test_allowed_routes_kind_case_insensitive() {
    use edgion_resources::resources::gateway::{RouteGroupKind, RouteNamespaces};

    let allowed = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("All".to_string()),
            selector: None,
        }),
        kinds: Some(vec![RouteGroupKind {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: "HTTPRoute".to_string(),
        }]),
    };

    assert!(check_allowed_routes(&Some(allowed), "default", "httproute", "default"));
}

#[test]
fn test_allowed_routes_empty_kinds_allows_all() {
    use edgion_resources::resources::gateway::RouteNamespaces;

    let allowed = AllowedRoutes {
        namespaces: Some(RouteNamespaces {
            from: Some("All".to_string()),
            selector: None,
        }),
        kinds: Some(vec![]), // empty list = no restriction
    };

    assert!(check_allowed_routes(
        &Some(allowed.clone()),
        "default",
        "HTTPRoute",
        "default"
    ));
    assert!(check_allowed_routes(&Some(allowed), "default", "GRPCRoute", "default"));
}

#[test]
fn listener_match_borrows_owner_until_final_selection() {
    for section in [Some("http"), None] {
        let index = GatewayInfoIndex::from_entries_for_test(vec![(
            "gw-ns/gw".into(),
            listener_entry("gw-ns", "gw", Some("http"), None, None),
        )]);
        let expected = &index.get("gw-ns/gw", "http").unwrap().info;
        let original_count = Arc::strong_count(expected);
        let matched = check_gateway_listener_match(
            &[resolved("gw-ns/gw", "gw-ns", section)],
            &index,
            "example.com",
            "gw-ns",
            "HTTPRoute",
        )
        .unwrap();
        assert!(Arc::ptr_eq(matched, expected));
        assert_eq!(Arc::strong_count(expected), original_count);
        let selected = Arc::clone(matched);
        assert_eq!(Arc::strong_count(expected), original_count + 1);
        drop(index);
        assert_eq!(selected.gateway_name(), "gw");
    }
}

use super::*;

#[test]
fn invalid_patterns_do_not_intersect_even_when_equal() {
    for invalid in ["*", "A.example.com", "127.0.0.1", "a..b"] {
        assert_eq!(compute_hostname_intersection(invalid, invalid), None);
    }
    assert_eq!(compute_hostname_intersection("a", "a"), Some("a".into()));
    assert_eq!(
        compute_hostname_intersection("*.127.0.0.1", "a.127.0.0.1"),
        Some("a.127.0.0.1".into())
    );
}

#[test]
fn test_intersection_both_concrete_equal() {
    assert_eq!(
        compute_hostname_intersection("example.com", "example.com"),
        Some("example.com".to_string())
    );
}

#[test]
fn test_intersection_both_concrete_different() {
    assert_eq!(compute_hostname_intersection("foo.com", "bar.com"), None);
}

#[test]
fn test_intersection_wildcard_listener_concrete_route() {
    assert_eq!(
        compute_hostname_intersection("*.wildcard.io", "foo.wildcard.io"),
        Some("foo.wildcard.io".to_string())
    );
}

#[test]
fn test_intersection_wildcard_listener_concrete_route_no_match() {
    assert_eq!(compute_hostname_intersection("*.wildcard.io", "wildcard.io"), None);
    assert_eq!(compute_hostname_intersection("*.bar.com", "foo.com"), None);
}

#[test]
fn test_intersection_concrete_listener_wildcard_route() {
    assert_eq!(
        compute_hostname_intersection("very.specific.com", "*.specific.com"),
        Some("very.specific.com".to_string())
    );
}

#[test]
fn test_intersection_concrete_listener_wildcard_route_no_match() {
    assert_eq!(compute_hostname_intersection("specific.com", "*.specific.com"), None);
}

#[test]
fn test_intersection_both_wildcards_same() {
    assert_eq!(
        compute_hostname_intersection("*.bar.com", "*.bar.com"),
        Some("*.bar.com".to_string())
    );
}

#[test]
fn test_intersection_both_wildcards_route_more_specific() {
    assert_eq!(
        compute_hostname_intersection("*.bar.com", "*.foo.bar.com"),
        Some("*.foo.bar.com".to_string())
    );
}

#[test]
fn test_intersection_both_wildcards_listener_more_specific() {
    assert_eq!(
        compute_hostname_intersection("*.foo.bar.com", "*.bar.com"),
        Some("*.foo.bar.com".to_string())
    );
}

#[test]
fn test_intersection_both_wildcards_no_overlap() {
    assert_eq!(compute_hostname_intersection("*.bar.com", "*.foo.com"), None);
}

#[test]
fn test_intersection_multilevel_wildcard() {
    assert_eq!(
        compute_hostname_intersection("*.example.com", "a.b.example.com"),
        Some("a.b.example.com".to_string())
    );
}

/// Helper to run resolve_for_listeners and return (hostnames, actions)
fn run_resolve_for_listeners(
    listeners: &[Option<String>],
    route_hostnames: Option<&Vec<String>>,
) -> (Vec<String>, Vec<String>) {
    let mut seen = HashSet::new();
    let mut all_effective = Vec::new();
    let mut actions = Vec::new();
    resolve_for_listeners(listeners, route_hostnames, &mut seen, &mut all_effective, &mut actions);
    if all_effective.is_empty() {
        all_effective.push("*".to_string());
    }
    (all_effective, actions)
}

#[test]
fn test_resolve_mixed_listeners_with_route_hostnames() {
    // Scenario 1: catch-all listener + hostname listener, route has hostnames
    // This is the bug scenario: should passthrough "example.org", not become "*"
    let listeners = vec![None, Some("second-example.org".to_string())];
    let route_hs = vec!["example.org".to_string()];
    let (hostnames, actions) = run_resolve_for_listeners(&listeners, Some(&route_hs));
    assert_eq!(hostnames, vec!["example.org"]);
    assert_eq!(actions, vec!["passthrough", "intersected"]);
}

#[test]
fn test_resolve_catchall_listener_route_has_hostnames() {
    // Scenario 2: single catch-all listener, route has hostnames -> passthrough
    let listeners = vec![None];
    let route_hs = vec!["foo.com".to_string()];
    let (hostnames, _actions) = run_resolve_for_listeners(&listeners, Some(&route_hs));
    assert_eq!(hostnames, vec!["foo.com"]);
}

#[test]
fn test_resolve_catchall_listener_route_no_hostnames() {
    // Scenario 3: single catch-all listener, route has no hostnames -> "*"
    let listeners = vec![None];
    let (hostnames, actions) = run_resolve_for_listeners(&listeners, None);
    assert_eq!(hostnames, vec!["*"]);
    assert_eq!(actions, vec!["catch-all"]);
}

#[test]
fn test_resolve_specific_listener_route_has_hostnames_intersection() {
    // Scenario 4: wildcard listener + route hostnames -> intersection
    let listeners = vec![Some("*.example.com".to_string())];
    let route_hs = vec!["foo.example.com".to_string()];
    let (hostnames, actions) = run_resolve_for_listeners(&listeners, Some(&route_hs));
    assert_eq!(hostnames, vec!["foo.example.com"]);
    assert_eq!(actions, vec!["intersected"]);
}

#[test]
fn test_resolve_specific_listener_route_no_hostnames_inherit() {
    // Scenario 5: specific listener, route has no hostnames -> inherit
    let listeners = vec![Some("example.com".to_string())];
    let (hostnames, actions) = run_resolve_for_listeners(&listeners, None);
    assert_eq!(hostnames, vec!["example.com"]);
    assert_eq!(actions, vec!["inherited:example.com"]);
}

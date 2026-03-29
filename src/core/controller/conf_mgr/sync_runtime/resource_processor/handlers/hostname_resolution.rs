//! Hostname Resolution for Routes
//!
//! Computes effective hostnames at the controller level by intersecting
//! route hostnames with Gateway listener hostnames per Gateway API spec.
//! The resolved hostnames are written into the route resource so the data
//! plane can use them directly without re-checking Gateway constraints.

use std::collections::HashSet;

use crate::types::prelude_resources::Gateway;
use crate::types::resources::common::ParentReference;

pub struct ResolvedHostnames {
    pub hostnames: Vec<String>,
    pub annotation: Option<String>,
}

/// Resolve effective hostnames for an HTTPRoute or GRPCRoute.
///
/// Logic:
/// 1. If route has `spec.hostnames` → compute intersection with each listener's hostname
/// 2. If route has no hostnames → inherit from listeners
/// 3. If listener has no hostname → catch-all `"*"`
/// 4. If Gateway not yet in registry → fallback to raw route hostnames
pub fn resolve_effective_hostnames(
    route_hostnames: Option<&Vec<String>>,
    parent_refs: &[ParentReference],
    route_ns: &str,
) -> ResolvedHostnames {
    if parent_refs.is_empty() {
        return ResolvedHostnames {
            hostnames: vec![],
            annotation: None,
        };
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut all_effective: Vec<String> = Vec::new();
    let mut actions: Vec<String> = Vec::new();

    for pr in parent_refs {
        let parent_group = pr.group.as_deref().unwrap_or("gateway.networking.k8s.io");
        let parent_kind = pr.kind.as_deref().unwrap_or("Gateway");
        if parent_group != "gateway.networking.k8s.io" || parent_kind != "Gateway" {
            continue;
        }

        let gw_ns = pr.namespace.as_deref().unwrap_or(route_ns);
        let gateway = match super::route_utils::lookup_gateway(gw_ns, &pr.name) {
            Some(gw) => gw,
            None => {
                if let Some(hs) = route_hostnames {
                    for h in hs {
                        if seen.insert(h.clone()) {
                            all_effective.push(h.clone());
                        }
                    }
                }
                actions.push("gateway-pending".to_string());
                continue;
            }
        };

        let matching_listeners = collect_matching_listeners(&gateway, pr);

        if matching_listeners.is_empty() {
            // No matching listeners — treat as if no Gateway constraints
            if let Some(route_hs) = route_hostnames {
                for h in route_hs {
                    if seen.insert(h.clone()) {
                        all_effective.push(h.clone());
                    }
                }
            }
            actions.push("no-listeners".to_string());
            continue;
        }

        // Per-listener resolution: compute effective hostnames for each
        // listener individually, then union all results.
        resolve_for_listeners(
            &matching_listeners,
            route_hostnames,
            &mut seen,
            &mut all_effective,
            &mut actions,
        );
    }

    if all_effective.is_empty() {
        all_effective.push("*".to_string());
    }

    let annotation = if actions.is_empty() {
        None
    } else {
        Some(actions.join(";"))
    };

    ResolvedHostnames {
        hostnames: all_effective,
        annotation,
    }
}

/// Resolve effective hostnames given a set of matching listener hostnames and route hostnames.
///
/// This is the core per-Gateway resolution logic extracted for testability.
/// It processes each listener hostname against the route hostnames and produces
/// the union of all effective hostnames with dedup.
fn resolve_for_listeners(
    matching_listeners: &[Option<String>],
    route_hostnames: Option<&Vec<String>>,
    seen: &mut HashSet<String>,
    all_effective: &mut Vec<String>,
    actions: &mut Vec<String>,
) {
    for listener_hostname in matching_listeners {
        match (listener_hostname.as_deref(), route_hostnames) {
            // Catch-all listener + route has hostnames -> passthrough
            (None, Some(route_hs)) if !route_hs.is_empty() => {
                for h in route_hs {
                    if seen.insert(h.clone()) {
                        all_effective.push(h.clone());
                    }
                }
                actions.push("passthrough".to_string());
            }
            // Catch-all listener + route has no hostnames -> "*"
            (None, _) => {
                if seen.insert("*".to_string()) {
                    all_effective.push("*".to_string());
                }
                actions.push("catch-all".to_string());
            }
            // Specific listener + route has hostnames -> intersection
            (Some(lh), Some(route_hs)) if !route_hs.is_empty() => {
                for route_hn in route_hs {
                    if let Some(intersected) = compute_hostname_intersection(lh, route_hn) {
                        if seen.insert(intersected.clone()) {
                            all_effective.push(intersected);
                        }
                    }
                }
                actions.push("intersected".to_string());
            }
            // Specific listener + route has no hostnames -> inherit
            (Some(lh), _) => {
                let lh_owned = lh.to_string();
                let action = format!("inherited:{}", &lh_owned);
                if seen.insert(lh_owned.clone()) {
                    all_effective.push(lh_owned);
                }
                actions.push(action);
            }
        }
    }
}

/// Collect hostnames from listeners that match a parentRef's sectionName/port filter.
///
/// Returns `Vec<Option<String>>` where each entry is one matching listener's hostname.
/// `None` means the listener has no hostname constraint (catch-all).
/// This preserves catch-all listener information that was previously lost.
fn collect_matching_listeners(gateway: &Gateway, pr: &ParentReference) -> Vec<Option<String>> {
    let listeners = match &gateway.spec.listeners {
        Some(ls) => ls,
        None => return Vec::new(),
    };

    let mut seen: HashSet<Option<String>> = HashSet::new();
    let mut result = Vec::new();
    for listener in listeners
        .iter()
        .filter(|l| pr.section_name.as_ref().is_none_or(|sn| l.name == *sn) && pr.port.is_none_or(|p| l.port == p))
    {
        let hostname = listener.hostname.as_ref().filter(|h| !h.is_empty()).cloned();
        if seen.insert(hostname.clone()) {
            result.push(hostname);
        }
    }
    result
}

/// Compute the intersection of a listener hostname and a route hostname.
///
/// Per Gateway API spec, the intersection is always the more specific hostname.
/// Returns None if they don't intersect.
///
/// | Listener          | Route             | Result              |
/// |-------------------|-------------------|---------------------|
/// | example.com       | example.com       | example.com         |
/// | *.wildcard.io     | foo.wildcard.io   | foo.wildcard.io     |
/// | very.specific.com | *.specific.com    | very.specific.com   |
/// | *.bar.com         | *.foo.bar.com     | *.foo.bar.com       |
/// | foo.com           | bar.com           | None                |
pub fn compute_hostname_intersection(listener_hn: &str, route_hn: &str) -> Option<String> {
    if listener_hn == route_hn {
        return Some(listener_hn.to_string());
    }

    let listener_is_wildcard = listener_hn.starts_with("*.");
    let route_is_wildcard = route_hn.starts_with("*.");

    match (listener_is_wildcard, route_is_wildcard) {
        (false, false) => {
            // Both concrete: must be equal (already checked above)
            None
        }
        (true, false) => {
            // Wildcard listener × concrete route
            let listener_suffix = &listener_hn[1..]; // ".example.com"
            if route_hn.ends_with(listener_suffix) && route_hn.len() > listener_suffix.len() {
                Some(route_hn.to_string())
            } else {
                None
            }
        }
        (false, true) => {
            // Concrete listener × wildcard route
            let route_suffix = &route_hn[1..]; // ".specific.com"
            if listener_hn.ends_with(route_suffix) && listener_hn.len() > route_suffix.len() {
                Some(listener_hn.to_string())
            } else {
                None
            }
        }
        (true, true) => {
            // Both wildcards: the more specific (longer suffix) wins
            let listener_suffix = &listener_hn[1..];
            let route_suffix = &route_hn[1..];
            if listener_suffix == route_suffix {
                Some(listener_hn.to_string())
            } else if route_suffix.ends_with(listener_suffix) && route_suffix.len() > listener_suffix.len() {
                // route is more specific: *.foo.bar.com under *.bar.com
                Some(route_hn.to_string())
            } else if listener_suffix.ends_with(route_suffix) && listener_suffix.len() > route_suffix.len() {
                // listener is more specific
                Some(listener_hn.to_string())
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

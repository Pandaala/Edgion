//! Hostname Resolution for Routes
//!
//! Computes effective hostnames at the controller level by intersecting
//! route hostnames with Gateway listener hostnames per Gateway API spec.
//! The resolved hostnames are written into the route resource so the data
//! plane can use them directly without re-checking Gateway constraints.

use std::collections::HashSet;

use crate::types::resources::common::ParentReference;
use crate::types::prelude_resources::Gateway;

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

        let listener_hostnames = collect_listener_hostnames(&gateway, pr);

        match route_hostnames {
            Some(route_hs) if !route_hs.is_empty() => {
                if listener_hostnames.is_empty() {
                    for route_hn in route_hs {
                        if seen.insert(route_hn.clone()) {
                            all_effective.push(route_hn.clone());
                        }
                    }
                    actions.push("passthrough".to_string());
                } else {
                    let mut intersected_any = false;
                    for listener_hn in &listener_hostnames {
                        for route_hn in route_hs {
                            if let Some(intersected) = compute_hostname_intersection(listener_hn, route_hn) {
                                if seen.insert(intersected.clone()) {
                                    all_effective.push(intersected);
                                }
                                intersected_any = true;
                            }
                        }
                    }
                    if intersected_any {
                        actions.push("intersected".to_string());
                    }
                }
            }
            _ => {
                if listener_hostnames.is_empty() {
                    if seen.insert("*".to_string()) {
                        all_effective.push("*".to_string());
                    }
                    actions.push("catch-all".to_string());
                } else {
                    for lh in &listener_hostnames {
                        if seen.insert(lh.clone()) {
                            all_effective.push(lh.clone());
                            actions.push(format!("inherited:{}", lh));
                        }
                    }
                }
            }
        }
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

/// Collect hostnames from listeners that match a parentRef's sectionName/port filter.
fn collect_listener_hostnames(gateway: &Gateway, pr: &ParentReference) -> Vec<String> {
    let listeners = match &gateway.spec.listeners {
        Some(ls) => ls,
        None => return Vec::new(),
    };

    let matching_listeners: Vec<_> = listeners
        .iter()
        .filter(|l| {
            pr.section_name.as_ref().is_none_or(|sn| l.name == *sn)
                && pr.port.map_or(true, |p| l.port == p)
        })
        .collect();

    let mut hostnames = Vec::new();
    for listener in matching_listeners {
        if let Some(hostname) = &listener.hostname {
            if !hostname.is_empty() && !hostnames.contains(hostname) {
                hostnames.push(hostname.clone());
            }
        }
    }
    hostnames
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
        assert_eq!(
            compute_hostname_intersection("*.wildcard.io", "wildcard.io"),
            None
        );
        assert_eq!(
            compute_hostname_intersection("*.bar.com", "foo.com"),
            None
        );
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
        assert_eq!(
            compute_hostname_intersection("specific.com", "*.specific.com"),
            None
        );
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
        assert_eq!(
            compute_hostname_intersection("*.bar.com", "*.foo.com"),
            None
        );
    }

    #[test]
    fn test_intersection_multilevel_wildcard() {
        assert_eq!(
            compute_hostname_intersection("*.example.com", "a.b.example.com"),
            Some("a.b.example.com".to_string())
        );
    }
}

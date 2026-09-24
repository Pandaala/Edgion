//! Hostname Resolution for Routes
//!
//! Computes effective hostnames at the controller level by intersecting
//! route hostnames with Gateway listener hostnames per Gateway API spec.
//! The resolved hostnames are written into the route resource so the data
//! plane can apply the eligibility constraint without changing authored route priority.

use std::collections::HashSet;

use crate::conf_mgr::ControllerRuntime;
use edgion_algo::matcher::radix_hostname::RadixHostnamePatternRef;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::common::ParentReference;

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
    runtime: &ControllerRuntime,
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
        let Some(gateway) = super::route_utils::lookup_gateway(gw_ns, &pr.name, runtime) else {
            if let Some(hs) = route_hostnames {
                for h in hs {
                    if seen.insert(h.clone()) {
                        all_effective.push(h.clone());
                    }
                }
            }
            actions.push("gateway-pending".to_string());
            continue;
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
    let Some(listeners) = &gateway.spec.listeners else {
        return Vec::new();
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
    let listener = RadixHostnamePatternRef::parse(listener_hn).ok()?;
    let route = RadixHostnamePatternRef::parse(route_hn).ok()?;
    listener.intersection(route).map(|pattern| pattern.as_str().to_owned())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/hostname_resolution_test.rs"]
mod tests;

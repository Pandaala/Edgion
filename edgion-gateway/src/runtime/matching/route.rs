//! Common route matching utilities for Gateway API
//!
//! This module provides shared functions for matching routes against
//! Gateway and Listener constraints, used by both HTTP and gRPC routes.

use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::{format_gateway_key, GatewayInfoIndex};
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::gateway::AllowedRoutes;

/// Check if request hostname matches listener hostname constraint.
///
/// Supports:
/// - Exact match (case-insensitive): "example.com" matches "example.com"
/// - Wildcard suffix match: "*.example.com" matches "api.example.com"
///   and "a.b.example.com" (per the Gateway API spec, any number of labels)
///
/// # TLS divergence
/// The Gateway API allows multi-level wildcard matching here, but TLS certificates follow
/// RFC 6125 §6.4.3 (single-label only). A cert with SAN `*.example.com` covers
/// `api.example.com`, but RFC 6125-compliant client TLS libraries reject the handshake
/// for `a.b.example.com` even when this function returns `true`. Use explicit SAN entries
/// for deep subdomains.
#[inline]
pub fn hostname_matches_listener(request_host: &str, listener_hostname: &str) -> bool {
    if listener_hostname.starts_with("*.") {
        let suffix = &listener_hostname[1..]; // ".example.com"
        if !request_host.ends_with(suffix) {
            return false;
        }
        let prefix_len = request_host.len() - suffix.len();
        prefix_len > 0
    } else {
        request_host.eq_ignore_ascii_case(listener_hostname)
    }
}

/// Check if a route is allowed by the listener's AllowedRoutes configuration.
///
/// Per Gateway API spec:
/// - namespaces.from: "All" (any namespace), "Same" (same as Gateway), "Selector" (label match)
/// - kinds: list of allowed route kinds (HTTPRoute, GRPCRoute, etc.)
#[inline]
pub fn check_allowed_routes(
    allowed_routes: &Option<AllowedRoutes>,
    route_namespace: &str,
    route_kind: &str,
    gateway_namespace: &str,
) -> bool {
    let Some(allowed) = allowed_routes else {
        return route_namespace == gateway_namespace;
    };

    // 1. Check namespace restrictions (default "Same")
    {
        let from = allowed
            .namespaces
            .as_ref()
            .and_then(|ns| ns.from.as_deref())
            .unwrap_or("Same");
        match from {
            "All" => {}
            "Same" => {
                if route_namespace != gateway_namespace {
                    return false;
                }
            }
            "Selector" => {
                // Selector is fully evaluated by the controller; trust its decision.
            }
            _ => {
                return false;
            }
        }
    }

    // 2. Check route kind restrictions
    if let Some(ref kinds) = allowed.kinds {
        if !kinds.is_empty() {
            let kind_allowed = kinds.iter().any(|k| k.kind.eq_ignore_ascii_case(route_kind));
            if !kind_allowed {
                return false;
            }
        }
    }

    true
}

/// Find the GatewayInfo that matches this route's parentRefs on the current listener port.
///
/// Uses the pre-built `GatewayInfoIndex` for O(1) lookups.
///
/// Validates:
/// 1. **Associate** the route with the correct Gateway (namespace + name match)
/// 2. **Locate the listener** via sectionName (fast path) or hostname isolation (fallback)
/// 3. **Enforce HTTP Listener Isolation** — hostname matching + more-specific check
/// 4. **AllowedRoutes** — namespace and kind filtering
///
/// Returns `Some(GatewayInfo)` for the first matching gateway/listener,
/// or `None` if no match is found.
pub fn check_gateway_listener_match(
    parent_refs: &[ParentReference],
    index: &GatewayInfoIndex,
    request_hostname: &str,
    route_ns: &str,
    route_kind: &str,
) -> Option<GatewayInfo> {
    for pr in parent_refs {
        let parent_ns = pr.namespace.as_deref().unwrap_or(route_ns);
        let gateway_key = format_gateway_key(parent_ns, &pr.name);

        // Fast path: sectionName → O(1) two-level HashMap lookup
        // Still must enforce hostname isolation per Gateway API spec.
        if let Some(section_name) = &pr.section_name {
            if let Some(entry) = index.get(&gateway_key, section_name) {
                if let Some(ref listener_host) = entry.hostname {
                    if !hostname_matches_listener(request_hostname, listener_host) {
                        continue;
                    }
                }
                // Check if a more-specific listener should take priority
                if let Some(gw) = index.get_gateway(&gateway_key) {
                    if gw.has_more_specific(request_hostname, entry.hostname.as_deref()) {
                        continue;
                    }
                }
                if !check_allowed_routes(&entry.allowed_routes, route_ns, route_kind, parent_ns) {
                    continue;
                }
                return Some(entry.info.clone());
            }
            continue;
        }

        // Slow path: no sectionName → hostname isolation
        let Some(gw) = index.get_gateway(&gateway_key) else {
            continue;
        };
        for entry in gw.iter() {
            if let Some(ref listener_host) = entry.hostname {
                if !hostname_matches_listener(request_hostname, listener_host) {
                    continue;
                }
            }

            if gw.has_more_specific(request_hostname, entry.hostname.as_deref()) {
                continue;
            }

            if !check_allowed_routes(&entry.allowed_routes, route_ns, route_kind, parent_ns) {
                continue;
            }

            return Some(entry.info.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

//! Common route matching utilities for Gateway API
//!
//! This module provides shared functions for matching routes against
//! Gateway and Listener constraints, used by both HTTP and gRPC routes.

use super::config_store::{get_global_gateway_config_store, GatewayInfo};
use crate::types::resources::common::ParentReference;
use crate::types::resources::gateway::AllowedRoutes;

/// Check if request hostname matches listener hostname constraint
///
/// Supports:
/// - Exact match (case-insensitive): "example.com" matches "example.com"
/// - Wildcard suffix match: "*.example.com" matches "api.example.com"
///   and "a.b.example.com" (per Gateway API spec, any number of labels)
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

/// Check if a route is allowed by the listener's AllowedRoutes configuration
///
/// Per Gateway API spec:
/// - namespaces.from: "All" (any namespace), "Same" (same as Gateway), "Selector" (label match)
/// - kinds: list of allowed route kinds (HTTPRoute, GRPCRoute, etc.)
///
/// Returns true if the route is allowed, false otherwise.
#[inline]
pub fn check_allowed_routes(
    allowed_routes: &Option<AllowedRoutes>,
    route_namespace: &str,
    route_kind: &str,
    gateway_namespace: &str,
) -> bool {
    // Per Gateway API spec, when allowedRoutes is not specified the default
    // is "Same" — only routes from the gateway's own namespace are allowed.
    let Some(allowed) = allowed_routes else {
        return route_namespace == gateway_namespace;
    };

    // 1. Check namespace restrictions (default "Same" when namespaces not specified)
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
                // Selector policy is fully evaluated by the controller, which has
                // access to namespace labels via NamespaceStore.  The gateway data
                // plane only sees routes whose parentRef was already accepted
                // (filter_accepted_parent_refs), so we trust the controller's
                // decision and allow the route through — same as "All".
            }
            _ => {
                tracing::warn!(from = %from, "Unknown AllowedRoutes.namespaces.from value");
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

/// Check if route's parent references match any of the provided gateway/listener contexts.
///
/// This function validates for each (parentRef, gatewayInfo) combination:
/// 1. Parent reference matches the gateway (namespace + name)
/// 2. SectionName matches the listener (if specified)
/// 3. Request hostname matches listener hostname constraint (if configured)
/// 4. Route is allowed by listener's AllowedRoutes (namespace and kind restrictions)
///
/// Returns `Some(GatewayInfo)` for the first gateway that passes all checks,
/// or `None` if no match is found.
pub fn check_gateway_listener_match(
    parent_refs: &[ParentReference],
    gateway_infos: &[GatewayInfo],
    request_hostname: &str,
    route_ns: &str,
    route_kind: &str,
    _route_name: &str,
) -> Option<GatewayInfo> {
    let config_store = get_global_gateway_config_store();

    for pr in parent_refs {
        let parent_ns = pr.namespace.as_deref().unwrap_or(route_ns);

        for gi in gateway_infos {
            let gateway_ns = gi.namespace_str();

            if parent_ns != gateway_ns || pr.name != gi.name {
                continue;
            }

            let listener_config = config_store.get_listener_config(gi);

            // Check sectionName matching
            match (&pr.section_name, &gi.listener_name) {
                (Some(section_name), Some(listener_name)) => {
                    if section_name != listener_name {
                        continue;
                    }
                }
                (Some(section_name), None) => {
                    if config_store.has_listener(parent_ns, &pr.name, section_name) {
                        return Some(gi.clone());
                    }
                    continue;
                }
                (None, _) => {}
            }

            if let Some(ref config) = listener_config {
                if let Some(ref listener_host) = config.hostname {
                    if !hostname_matches_listener(request_hostname, listener_host) {
                        continue;
                    }
                }

                if !check_allowed_routes(&config.allowed_routes, route_ns, route_kind, gateway_ns) {
                    continue;
                }
            }

            return Some(gi.clone());
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
        use crate::types::resources::gateway::RouteNamespaces;

        let allowed = AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Same".to_string()),
                selector: None,
            }),
            kinds: None,
        };

        // Same namespace - allowed
        assert!(check_allowed_routes(
            &Some(allowed.clone()),
            "default",
            "HTTPRoute",
            "default"
        ));

        // Different namespace - not allowed
        assert!(!check_allowed_routes(&Some(allowed), "other", "HTTPRoute", "default"));
    }

    #[test]
    fn test_allowed_routes_selector_trusts_controller() {
        use crate::types::resources::gateway::RouteNamespaces;

        let allowed = AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Selector".to_string()),
                selector: Some(serde_json::json!({ "matchLabels": { "env": "prod" } })),
            }),
            kinds: None,
        };

        // Same namespace — allowed
        assert!(check_allowed_routes(
            &Some(allowed.clone()),
            "default",
            "HTTPRoute",
            "default"
        ));

        // Cross-namespace — allowed on gateway side (controller already validated)
        assert!(check_allowed_routes(
            &Some(allowed),
            "other",
            "HTTPRoute",
            "default"
        ));
    }
}

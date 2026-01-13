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
/// - Wildcard match: "*.example.com" matches "api.example.com" (single level only)
///
/// Per Gateway API spec, wildcards match only one DNS label (no dots in the matched part).
#[inline]
pub fn hostname_matches_listener(request_host: &str, listener_hostname: &str) -> bool {
    if listener_hostname.starts_with("*.") {
        // Wildcard match: *.example.com matches api.example.com
        let suffix = &listener_hostname[1..]; // ".example.com"
        if !request_host.ends_with(suffix) {
            return false;
        }
        // Check that the prefix (before suffix) has no dots (single label only)
        let prefix_len = request_host.len() - suffix.len();
        if prefix_len == 0 {
            return false; // ".example.com" should not match "*.example.com"
        }
        !request_host[..prefix_len].contains('.')
    } else {
        // Exact match (case-insensitive)
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
    // If no AllowedRoutes configured, allow all (Gateway API default behavior)
    let Some(allowed) = allowed_routes else {
        return true;
    };

    // 1. Check namespace restrictions
    if let Some(ref ns_config) = allowed.namespaces {
        let from = ns_config.from.as_deref().unwrap_or("Same");
        match from {
            "All" => {
                // Allow routes from any namespace
            }
            "Same" => {
                // Only allow routes from same namespace as Gateway
                if route_namespace != gateway_namespace {
                    tracing::trace!(
                        route_ns = %route_namespace,
                        gateway_ns = %gateway_namespace,
                        "Route namespace does not match Gateway namespace (AllowedRoutes.namespaces.from=Same)"
                    );
                    return false;
                }
            }
            "Selector" => {
                // Label selector matching - would need namespace labels
                // For now, treat as allow (full implementation requires k8s API access)
                tracing::trace!(
                    "AllowedRoutes.namespaces.from=Selector not fully implemented, allowing"
                );
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
                tracing::trace!(
                    route_kind = %route_kind,
                    allowed_kinds = ?kinds.iter().map(|k| &k.kind).collect::<Vec<_>>(),
                    "Route kind not in AllowedRoutes.kinds"
                );
                return false;
            }
        }
    }

    true
}

/// Check if route's parent references match current gateway and listener constraints
///
/// This function validates:
/// 1. Parent reference matches current gateway (namespace + name)
/// 2. SectionName matches current listener (if specified)
/// 3. Request hostname matches listener hostname constraint (if configured)
/// 4. Route is allowed by listener's AllowedRoutes (namespace and kind restrictions)
///
/// Returns true if at least one parent_ref passes all checks.
pub fn check_gateway_listener_match(
    parent_refs: &[ParentReference],
    gateway_info: &GatewayInfo,
    request_hostname: &str,
    route_ns: &str,
    route_kind: &str,
    route_name: &str,
) -> bool {
    let config_store = get_global_gateway_config_store();
    let gateway_ns = gateway_info.namespace_str();

    parent_refs.iter().any(|pr| {
        // Get parent gateway namespace (default to route's namespace per Gateway API spec)
        let parent_ns = pr.namespace.as_deref().unwrap_or(route_ns);

        // Check if parent reference matches current gateway
        // A Route can have multiple parentRefs pointing to different Gateways,
        // so not matching current gateway is normal - just skip this parentRef
        if parent_ns != gateway_ns || pr.name != gateway_info.name {
            return false;
        }

        // Dynamically get current listener configuration (supports hot-reload)
        let listener_config = config_store.get_listener_config(gateway_info);

        // Check sectionName matching
        match (&pr.section_name, &gateway_info.listener_name) {
            // Route specifies sectionName - must match current listener exactly
            (Some(section_name), Some(listener_name)) => {
                if section_name != listener_name {
                    return false;
                }
            }
            // Route specifies sectionName but we don't have listener context
            // This shouldn't happen in normal EdgionHttp flow, but handle gracefully
            (Some(section_name), None) => {
                // Just verify the listener exists - caller should have listener context
                return config_store.has_listener(parent_ns, &pr.name, section_name);
            }
            // Route doesn't specify sectionName - can attach to any listener
            (None, _) => {}
        }

        // Check listener constraints (hostname and AllowedRoutes)
        if let Some(ref config) = listener_config {
            // Check hostname constraint
            if let Some(ref listener_host) = config.hostname {
                if !hostname_matches_listener(request_hostname, listener_host) {
                    tracing::trace!(
                        request_host = %request_hostname,
                        listener_host = %listener_host,
                        route_ns = %route_ns,
                        route_name = %route_name,
                        "Request hostname does not match listener hostname constraint"
                    );
                    return false;
                }
            }

            // Check AllowedRoutes constraint
            if !check_allowed_routes(&config.allowed_routes, route_ns, route_kind, gateway_ns) {
                tracing::trace!(
                    route_ns = %route_ns,
                    route_name = %route_name,
                    gateway_ns = %gateway_ns,
                    "Route not allowed by listener's AllowedRoutes"
                );
                return false;
            }
        }

        true
    })
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
        // Wildcard should match subdomain
        assert!(hostname_matches_listener("api.example.com", "*.example.com"));
        assert!(hostname_matches_listener("foo.example.com", "*.example.com"));

        // Wildcard should NOT match the domain itself
        assert!(!hostname_matches_listener("example.com", "*.example.com"));

        // Wildcard should NOT match different domain
        assert!(!hostname_matches_listener("api.other.com", "*.example.com"));

        // Wildcard should NOT match multi-level subdomain
        assert!(!hostname_matches_listener(
            "foo.api.example.com",
            "*.example.com"
        ));
    }

    #[test]
    fn test_allowed_routes_none() {
        // No AllowedRoutes means allow all
        assert!(check_allowed_routes(&None, "ns1", "HTTPRoute", "ns2"));
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
        assert!(!check_allowed_routes(
            &Some(allowed),
            "other",
            "HTTPRoute",
            "default"
        ));
    }
}

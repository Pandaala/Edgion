//! Common route matching utilities for Gateway API
//!
//! This module provides shared functions for matching routes against
//! Gateway and Listener constraints, used by both HTTP and gRPC routes.

use crate::core::gateway::runtime::store::config::GatewayInfo;
use crate::core::gateway::runtime::store::port_gateway_info::{format_gateway_key, GatewayInfoIndex};
use crate::types::resources::common::ParentReference;

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

/// Find the GatewayInfo that matches this route's parentRefs on the current listener port.
///
/// Uses the pre-built `GatewayInfoIndex` for O(1) lookups, eliminating the
/// previous O(parentRefs × gateway_infos) double loop and cross-store queries.
///
/// The controller has already validated parentRef acceptance (sectionName existence,
/// AllowedRoutes namespace/kind policy, hostname intersection) and filtered out
/// rejected parentRefs via `filter_accepted_parent_refs`. The gateway data plane
/// therefore only needs to:
///
/// 1. **Associate** the route with the correct Gateway (namespace + name match)
/// 2. **Locate the listener** via sectionName (fast path) or hostname isolation (fallback)
/// 3. **Enforce HTTP Listener Isolation** — when no sectionName is specified, verify
///    the request hostname matches the listener's hostname constraint and that no
///    more-specific listener on the same gateway+port should take priority.
///
/// Returns `Some(GatewayInfo)` for the first matching gateway/listener,
/// or `None` if no match is found.
pub fn check_gateway_listener_match(
    parent_refs: &[ParentReference],
    index: &GatewayInfoIndex,
    request_hostname: &str,
    route_ns: &str,
) -> Option<GatewayInfo> {
    for pr in parent_refs {
        let parent_ns = pr.namespace.as_deref().unwrap_or(route_ns);
        let gateway_key = format_gateway_key(parent_ns, &pr.name);

        // Fast path: sectionName → O(1) two-level HashMap lookup
        if let Some(section_name) = &pr.section_name {
            if let Some(entry) = index.get(&gateway_key, section_name) {
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
}

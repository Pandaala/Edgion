//! Common route matching utilities for Gateway API
//!
//! This module provides shared functions for matching routes against
//! Gateway and Listener constraints, used by both HTTP and gRPC routes.

use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use edgion_algo::matcher::radix_hostname::radix_hostname_matches;
use edgion_resources::resources::gateway::AllowedRoutes;
use std::sync::Arc;

/// A `ParentReference` with everything [`check_gateway_listener_match`] needs,
/// resolved once when the route snapshot is built.
///
/// Both inputs to the `GatewayInfoIndex` lookup key are immutable snapshot data:
/// the `ParentReference` fields, and the owning route's namespace used as the
/// Gateway API fallback when `namespace` is omitted. Deriving the key per
/// request per route candidate therefore rebuilds identical bytes on every
/// request; resolving it here makes the request path a pure read.
///
/// Only the *key* can be resolved here. `GatewayInfoIndex` is rebuilt and
/// swapped on every Gateway change, independently of the route snapshot, so an
/// index slot or a borrowed `ListenerEntry` must never be cached.
#[derive(Clone, Debug)]
pub struct ResolvedParentRef {
    /// `GatewayInfoIndex` lookup key, produced by `format_gateway_key`.
    pub gateway_key: String,
    /// `ParentReference::namespace` falling back to the route's namespace, per
    /// the Gateway API default ("When unspecified, this refers to the local
    /// namespace of the Route"). This is the *Gateway's* namespace, which
    /// [`check_allowed_routes`] takes separately from the route's own.
    pub parent_ns: String,
    /// Copied verbatim from the source `ParentReference`.
    pub section_name: Option<String>,
    /// Declared parent port constraint, checked against the logical listener.
    pub port: Option<i32>,
}

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
    radix_hostname_matches(request_host, listener_hostname)
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
/// 2. **Locate the listener** via sectionName (fast path) or hostname isolation (fallback),
///    enforcing the optional declared port in either path
/// 3. **Enforce HTTP Listener Isolation** — hostname matching + more-specific check
/// 4. **AllowedRoutes** — namespace and kind filtering
///
/// Returns the shared `Arc<GatewayInfo>` for the first matching gateway/listener,
/// or `None` if no match is found.
pub fn check_gateway_listener_match(
    parent_refs: &[ResolvedParentRef],
    index: &GatewayInfoIndex,
    request_hostname: &str,
    route_ns: &str,
    route_kind: &str,
) -> Option<Arc<GatewayInfo>> {
    for pr in parent_refs {
        // `route_ns` is the ROUTE's namespace and `parent_ns` is the GATEWAY's.
        // `check_allowed_routes` compares them, so they must stay distinct: they
        // are equal only in the same-namespace case, and conflating them would
        // silently admit cross-namespace routes that `AllowedRoutes: Same` and
        // the no-`allowedRoutes` default must reject.
        let gateway_key = pr.gateway_key.as_str();
        let parent_ns = pr.parent_ns.as_str();
        let Some(gw) = index.get_gateway(gateway_key) else {
            continue;
        };

        // Fast path: sectionName → O(1) two-level HashMap lookup
        // Still must enforce hostname isolation per Gateway API spec.
        if let Some(section_name) = &pr.section_name {
            if let Some(entry) = gw.get(section_name) {
                if pr.port.is_some_and(|port| port != entry.port) {
                    continue;
                }
                if let Some(ref listener_host) = entry.hostname {
                    if !hostname_matches_listener(request_hostname, listener_host) {
                        continue;
                    }
                }
                // Check if a more-specific listener should take priority
                if index.has_more_specific(request_hostname, entry.hostname.as_deref()) {
                    continue;
                }
                if !check_allowed_routes(&entry.allowed_routes, route_ns, route_kind, parent_ns) {
                    continue;
                }
                return Some(Arc::clone(&entry.info));
            }
            continue;
        }

        // Slow path: no sectionName → hostname isolation
        for entry in gw.iter() {
            if pr.port.is_some_and(|port| port != entry.port) {
                continue;
            }
            if let Some(ref listener_host) = entry.hostname {
                if !hostname_matches_listener(request_hostname, listener_host) {
                    continue;
                }
            }

            if index.has_more_specific(request_hostname, entry.hostname.as_deref()) {
                continue;
            }

            if !check_allowed_routes(&entry.allowed_routes, route_ns, route_kind, parent_ns) {
                continue;
            }

            return Some(Arc::clone(&entry.info));
        }
    }

    None
}

#[cfg(test)]
#[path = "../../../tests/unit/runtime/matching/route_test.rs"]
mod tests;

//! Shared utilities for Route and Gateway handlers
//!
//! Deduplicates common logic used across gateway.rs, http_route.rs,
//! grpc_route.rs, and other route handlers.

use crate::core::conf_mgr::PROCESSOR_REGISTRY;
use crate::types::prelude_resources::Gateway;
use crate::types::resources::gateway::AllowedRoutes;

/// Check if a listener's namespace policy allows a route from the given namespace.
///
/// Per Gateway API spec, the default (no AllowedRoutes or no namespaces field)
/// is "Same" — only routes from the same namespace as the Gateway are allowed.
pub fn listener_allows_route_namespace(
    allowed_routes: &Option<AllowedRoutes>,
    route_ns: &str,
    gateway_ns: &str,
) -> bool {
    let Some(allowed) = allowed_routes else {
        return route_ns == gateway_ns;
    };
    let Some(namespaces) = &allowed.namespaces else {
        return route_ns == gateway_ns;
    };
    match namespaces.from.as_deref().unwrap_or("Same") {
        "All" => true,
        "Same" => route_ns == gateway_ns,
        "Selector" => true,
        _ => route_ns == gateway_ns,
    }
}

/// Check if a listener hostname and a route hostname intersect per Gateway API spec.
///
/// Wildcards in listener hostname: `*.foo.com` matches `bar.foo.com`
/// but not `foo.com` or `baz.bar.foo.com` (single DNS label only).
pub fn hostnames_intersect(listener_hn: &str, route_hn: &str) -> bool {
    if listener_hn == route_hn {
        return true;
    }
    if let Some(suffix) = listener_hn.strip_prefix("*.") {
        if let Some(rest) = route_hn.strip_suffix(suffix) {
            let label = rest.strip_suffix('.').unwrap_or(rest);
            return !label.is_empty() && !label.contains('.');
        }
    }
    false
}

/// Look up a Gateway resource from the processor registry.
pub fn lookup_gateway(namespace: &str, name: &str) -> Option<Gateway> {
    let processor = PROCESSOR_REGISTRY.get("Gateway")?;
    let (json, _) = processor.as_watch_obj().list_json().ok()?;
    let gateways: Vec<Gateway> = serde_json::from_str(&json).ok()?;

    gateways.into_iter().find(|gw| {
        gw.metadata.namespace.as_deref().unwrap_or("default") == namespace
            && gw.metadata.name.as_deref().unwrap_or("") == name
    })
}

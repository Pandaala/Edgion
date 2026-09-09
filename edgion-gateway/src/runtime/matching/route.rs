//! Common route matching utilities for Gateway API
//!
//! This module provides shared functions for matching routes against
//! Gateway and Listener constraints, used by both HTTP and gRPC routes.

use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
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
    // `ParentReference::port` is deliberately absent: its only consumer is
    // `is_parent_ref_accepted`, which runs inside `filter_accepted_parent_refs`
    // on the config-publish path, strictly before resolution. Add it here if
    // request-time parentRef port disambiguation is ever introduced.
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

        // Fast path: sectionName → O(1) two-level HashMap lookup
        // Still must enforce hostname isolation per Gateway API spec.
        if let Some(section_name) = &pr.section_name {
            if let Some(entry) = index.get(gateway_key, section_name) {
                if let Some(ref listener_host) = entry.hostname {
                    if !hostname_matches_listener(request_hostname, listener_host) {
                        continue;
                    }
                }
                // Check if a more-specific listener should take priority
                if let Some(gw) = index.get_gateway(gateway_key) {
                    if gw.has_more_specific(request_hostname, entry.hostname.as_deref()) {
                        continue;
                    }
                }
                if !check_allowed_routes(&entry.allowed_routes, route_ns, route_kind, parent_ns) {
                    continue;
                }
                return Some(Arc::clone(&entry.info));
            }
            continue;
        }

        // Slow path: no sectionName → hostname isolation
        let Some(gw) = index.get_gateway(gateway_key) else {
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

            return Some(Arc::clone(&entry.info));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::runtime::store::listener_gateway_info::ListenerEntry;

    fn resolved(gateway_key: &str, parent_ns: &str, section_name: Option<&str>) -> ResolvedParentRef {
        ResolvedParentRef {
            gateway_key: gateway_key.to_string(),
            parent_ns: parent_ns.to_string(),
            section_name: section_name.map(str::to_string),
        }
    }

    fn listener_entry(
        gateway_ns: &str,
        gateway_name: &str,
        listener_name: Option<&str>,
        hostname: Option<&str>,
        allowed_routes: Option<AllowedRoutes>,
    ) -> ListenerEntry {
        ListenerEntry {
            info: Arc::new(GatewayInfo::new(
                Some(gateway_ns.to_string()),
                gateway_name.to_string(),
                listener_name.map(str::to_string),
                None,
                None,
                None,
                Vec::new(),
                None,
            )),
            hostname: hostname.map(str::to_string),
            allowed_routes,
        }
    }

    /// A cross-namespace parentRef against a listener with no `allowedRoutes`
    /// must be rejected: `check_allowed_routes` falls back to
    /// `route_namespace == gateway_namespace`.
    ///
    /// This is the regression guard for the aliasing trap in this function.
    /// `route_ns` (the ROUTE's namespace) and `ResolvedParentRef::parent_ns`
    /// (the GATEWAY's) are equal in the same-namespace case, so reusing one for
    /// both compiles and passes every same-namespace test while silently
    /// admitting the cross-namespace route this test rejects.
    #[test]
    fn cross_namespace_parent_ref_is_rejected_without_allowed_routes() {
        let index = GatewayInfoIndex::from_entries_for_test(vec![(
            "gw-ns/gw".to_string(),
            listener_entry("gw-ns", "gw", Some("http"), None, None),
        )]);
        let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", Some("http"))];

        // Route lives in `route-ns`, Gateway in `gw-ns` -> denied.
        assert!(
            check_gateway_listener_match(&parent_refs, &index, "example.com", "route-ns", "HTTPRoute").is_none(),
            "a cross-namespace route must not attach without allowedRoutes"
        );

        // Same reference, route now co-located with the Gateway -> allowed.
        assert!(
            check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute").is_some(),
            "a same-namespace route must still attach"
        );
    }

    /// The same guard on the no-sectionName path, which iterates listeners
    /// instead of taking the `by_name` fast path.
    #[test]
    fn cross_namespace_parent_ref_is_rejected_on_the_no_section_name_path() {
        let index = GatewayInfoIndex::from_entries_for_test(vec![(
            "gw-ns/gw".to_string(),
            listener_entry("gw-ns", "gw", Some("http"), None, None),
        )]);
        let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", None)];

        assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "route-ns", "HTTPRoute").is_none());
        assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute").is_some());
    }

    /// `from: "All"` is what actually authorizes a cross-namespace attachment,
    /// and it must still be honoured after the refactor.
    #[test]
    fn cross_namespace_parent_ref_is_allowed_by_from_all() {
        use edgion_resources::resources::gateway::RouteNamespaces;

        let allowed = AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("All".to_string()),
                selector: None,
            }),
            kinds: None,
        };
        let index = GatewayInfoIndex::from_entries_for_test(vec![(
            "gw-ns/gw".to_string(),
            listener_entry("gw-ns", "gw", Some("http"), None, Some(allowed)),
        )]);
        let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", Some("http"))];

        assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "route-ns", "HTTPRoute").is_some());
    }

    /// The resolved key is the only thing consulted for the index lookup, so a
    /// key naming no Gateway simply misses instead of matching something else.
    #[test]
    fn an_unknown_gateway_key_matches_nothing() {
        let index = GatewayInfoIndex::from_entries_for_test(vec![(
            "gw-ns/gw".to_string(),
            listener_entry("gw-ns", "gw", Some("http"), None, None),
        )]);
        let parent_refs = vec![resolved("gw-ns/absent", "gw-ns", Some("http"))];

        assert!(check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute").is_none());
    }

    /// parentRefs are tried in order and the first match wins.
    #[test]
    fn the_first_matching_parent_ref_wins() {
        let index = GatewayInfoIndex::from_entries_for_test(vec![
            (
                "gw-ns/first".to_string(),
                listener_entry("gw-ns", "first", Some("http"), None, None),
            ),
            (
                "gw-ns/second".to_string(),
                listener_entry("gw-ns", "second", Some("http"), None, None),
            ),
        ]);
        let parent_refs = vec![
            resolved("gw-ns/first", "gw-ns", Some("http")),
            resolved("gw-ns/second", "gw-ns", Some("http")),
        ];

        let matched = check_gateway_listener_match(&parent_refs, &index, "example.com", "gw-ns", "HTTPRoute")
            .expect("one of the two gateways must match");
        assert_eq!(matched.gateway_name(), "first");
    }

    /// Listener hostname isolation is unaffected by the refactor.
    #[test]
    fn listener_hostname_isolation_still_applies() {
        let index = GatewayInfoIndex::from_entries_for_test(vec![(
            "gw-ns/gw".to_string(),
            listener_entry("gw-ns", "gw", Some("http"), Some("*.example.com"), None),
        )]);
        let parent_refs = vec![resolved("gw-ns/gw", "gw-ns", Some("http"))];

        assert!(check_gateway_listener_match(&parent_refs, &index, "api.example.com", "gw-ns", "HTTPRoute").is_some());
        assert!(check_gateway_listener_match(&parent_refs, &index, "other.org", "gw-ns", "HTTPRoute").is_none());
    }

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

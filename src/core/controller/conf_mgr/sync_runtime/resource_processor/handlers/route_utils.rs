//! Shared utilities for Route and Gateway handlers
//!
//! Deduplicates common logic used across gateway.rs, http_route.rs,
//! grpc_route.rs, and other route handlers.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{AcceptedError, ResolvedRefsError};
use crate::core::controller::conf_mgr::PROCESSOR_REGISTRY;
use crate::types::prelude_resources::Gateway;
use crate::types::resources::common::{ParentReference, RefDenied};
use crate::types::resources::gateway::AllowedRoutes;
use crate::types::resources::http_route::RouteParentStatus;

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
        "Selector" => {
            let Some(selector) = &namespaces.selector else {
                // Selector mode but no selector specified: per spec, empty selector matches all
                return true;
            };
            let store = super::super::namespace_store::get_namespace_store();
            let Some(ns_labels) = store.get_labels(route_ns) else {
                // Namespace not found in store (not yet synced or non-K8s mode).
                // Fall back to Same policy: same-ns is safe, cross-ns is denied.
                tracing::warn!(
                    route_ns = %route_ns,
                    gateway_ns = %gateway_ns,
                    "Namespace labels not found for Selector evaluation, falling back to Same"
                );
                return route_ns == gateway_ns;
            };
            super::super::namespace_store::label_selector_matches(selector, &ns_labels)
        }
        _ => route_ns == gateway_ns,
    }
}

/// Check if a listener hostname and a route hostname intersect per Gateway API spec.
///
/// Per spec, wildcard `*.foo.com` matches any hostname ending with `.foo.com`
/// (multi-level: `a.foo.com`, `a.b.foo.com`, etc.) but NOT `foo.com` itself.
///
/// Covers all 4 combinations:
/// 1. Both concrete: exact equality
/// 2. Wildcard listener × concrete route: route is under listener's wildcard
/// 3. Concrete listener × wildcard route: listener is under route's wildcard
/// 4. Both wildcards: one suffix contains the other
pub fn hostnames_intersect(listener_hn: &str, route_hn: &str) -> bool {
    if listener_hn == route_hn {
        return true;
    }

    if let Some(listener_suffix) = listener_hn.strip_prefix("*.") {
        if let Some(route_suffix) = route_hn.strip_prefix("*.") {
            // Both wildcards: *.a.com vs *.b.a.com
            return route_suffix == listener_suffix
                || route_suffix.ends_with(&format!(".{}", listener_suffix))
                || listener_suffix.ends_with(&format!(".{}", route_suffix));
        }
        // Wildcard listener vs concrete route: *.bar.com vs foo.bar.com
        return route_hn.ends_with(&format!(".{}", listener_suffix));
    }

    if let Some(route_suffix) = route_hn.strip_prefix("*.") {
        // Concrete listener vs wildcard route: very.specific.com vs *.specific.com
        return listener_hn.ends_with(&format!(".{}", route_suffix));
    }

    false
}

/// Check if a Service resource exists in the processor registry.
pub fn service_exists(namespace: &str, name: &str) -> bool {
    let Some(processor) = PROCESSOR_REGISTRY.get("Service") else {
        return true;
    };
    processor.contains_key(&format!("{}/{}", namespace, name))
}

/// Validate backend refs: check kind is "Service" and the Service exists.
///
/// Each tuple is `(kind, namespace, name)` extracted from the route's backend refs.
/// Returns typed errors for ResolvedRefs condition generation.
pub fn validate_backend_refs(
    route_ns: &str,
    backend_refs: &[(Option<&str>, Option<&str>, &str)],
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for &(kind, namespace, name) in backend_refs {
        let kind_str = kind.unwrap_or("Service");
        if kind_str != "Service" {
            errors.push(ResolvedRefsError::InvalidKind {
                kind: kind_str.to_string(),
                name: name.to_string(),
            });
            continue;
        }
        let backend_ns = namespace.unwrap_or(route_ns);
        if !service_exists(backend_ns, name) {
            errors.push(ResolvedRefsError::BackendNotFound {
                namespace: backend_ns.to_string(),
                name: name.to_string(),
            });
        }
    }
    errors
}

/// Collect RefNotPermitted errors from ref_denied fields on backend refs.
pub fn collect_ref_denied_errors(ref_denied_list: &[Option<&RefDenied>]) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for ref_denied in ref_denied_list.iter().flatten() {
        errors.push(ResolvedRefsError::RefNotPermitted {
            target_namespace: ref_denied.target_namespace.clone(),
            target_name: ref_denied.target_name.clone(),
        });
    }
    errors
}

/// Validate a parent ref for the Accepted condition.
///
/// Checks namespace policy and (optionally) hostname intersection.
/// For L4 routes without hostnames, pass `None` to skip hostname checks.
pub fn validate_parent_ref_accepted(
    route_ns: &str,
    parent_ref: &ParentReference,
    route_hostnames: Option<&Vec<String>>,
) -> Vec<AcceptedError> {
    let mut errors = Vec::new();

    let parent_group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
    if parent_group != "gateway.networking.k8s.io" {
        return errors;
    }
    let parent_kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
    if parent_kind != "Gateway" {
        return errors;
    }

    let parent_ns = parent_ref.namespace.as_deref().unwrap_or(route_ns);
    let parent_name = &parent_ref.name;

    let Some(gateway) = lookup_gateway(parent_ns, parent_name) else {
        return errors;
    };

    let empty_listeners = Vec::new();
    let listeners = gateway.spec.listeners.as_ref().unwrap_or(&empty_listeners);

    if let Some(section_name) = &parent_ref.section_name {
        let has_listener = listeners.iter().any(|l| {
            l.name == *section_name && parent_ref.port.map_or(true, |p| l.port == p)
        });
        if !has_listener {
            errors.push(AcceptedError::NoMatchingParent {
                section_name: section_name.clone(),
            });
            return errors;
        }
    } else if let Some(port) = parent_ref.port {
        let has_listener = listeners.iter().any(|l| l.port == port);
        if !has_listener {
            errors.push(AcceptedError::NoMatchingParent {
                section_name: format!("port:{}", port),
            });
            return errors;
        }
    }

    let matching_listeners: Vec<_> = listeners
        .iter()
        .filter(|l| {
            parent_ref.section_name.as_ref().is_none_or(|sn| l.name == *sn)
                && parent_ref.port.map_or(true, |p| l.port == p)
        })
        .collect();

    let ns_allowed = matching_listeners
        .iter()
        .any(|l| listener_allows_route_namespace(&l.allowed_routes, route_ns, parent_ns));

    if !ns_allowed {
        errors.push(AcceptedError::NotAllowedByListeners {
            route_ns: route_ns.to_string(),
        });
        return errors;
    }

    if let Some(route_hs) = route_hostnames {
        if !route_hs.is_empty() {
            let hostname_match = matching_listeners.iter().any(|listener| {
                match &listener.hostname {
                    None => true,
                    Some(listener_hn) => route_hs
                        .iter()
                        .any(|route_hn| hostnames_intersect(listener_hn, route_hn)),
                }
            });
            if !hostname_match {
                errors.push(AcceptedError::NoMatchingListenerHostname {
                    hostnames: route_hs.clone(),
                });
            }
        }
    }

    errors
}

/// Remove stale parent statuses for parents no longer in the spec.
///
/// Mirrors the stale listener status cleanup in Gateway handler.
pub fn retain_current_parent_statuses(parents: &mut Vec<RouteParentStatus>, parent_refs: &[ParentReference]) {
    parents.retain(|ps| {
        parent_refs.iter().any(|pr| {
            pr.name == ps.parent_ref.name
                && pr.namespace == ps.parent_ref.namespace
                && pr.section_name == ps.parent_ref.section_name
                && pr.port == ps.parent_ref.port
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hostnames_intersect_concrete() {
        assert!(hostnames_intersect("foo.com", "foo.com"));
        assert!(!hostnames_intersect("foo.com", "bar.com"));
    }

    #[test]
    fn test_hostnames_intersect_wildcard_listener_concrete_route() {
        assert!(hostnames_intersect("*.wildcard.io", "foo.wildcard.io"));
        assert!(hostnames_intersect("*.bar.com", "a.b.bar.com"));
        assert!(!hostnames_intersect("*.wildcard.io", "wildcard.io"));
        assert!(!hostnames_intersect("*.bar.com", "foo.com"));
    }

    #[test]
    fn test_hostnames_intersect_concrete_listener_wildcard_route() {
        assert!(hostnames_intersect("very.specific.com", "*.specific.com"));
        assert!(!hostnames_intersect("specific.com", "*.specific.com"));
    }

    #[test]
    fn test_hostnames_intersect_both_wildcards() {
        assert!(hostnames_intersect("*.bar.com", "*.foo.bar.com"));
        assert!(hostnames_intersect("*.foo.bar.com", "*.bar.com"));
        assert!(hostnames_intersect("*.bar.com", "*.bar.com"));
        assert!(!hostnames_intersect("*.bar.com", "*.foo.com"));
    }

    // ---- listener_allows_route_namespace tests ----

    use crate::types::resources::gateway::{AllowedRoutes, RouteNamespaces};

    #[test]
    fn test_ns_policy_none_defaults_to_same() {
        assert!(listener_allows_route_namespace(&None, "ns1", "ns1"));
        assert!(!listener_allows_route_namespace(&None, "ns1", "ns2"));
    }

    #[test]
    fn test_ns_policy_all() {
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("All".into()),
                selector: None,
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "any-ns", "gw-ns"));
    }

    #[test]
    fn test_ns_policy_same() {
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Same".into()),
                selector: None,
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "ns1", "ns1"));
        assert!(!listener_allows_route_namespace(&allowed, "ns1", "ns2"));
    }

    #[test]
    fn test_ns_policy_selector_no_selector_field() {
        // Selector mode but no selector specified → matches all
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Selector".into()),
                selector: None,
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "any-ns", "gw-ns"));
    }

    #[test]
    fn test_ns_policy_selector_falls_back_to_same_when_labels_missing() {
        // When namespace labels are unavailable (FileSystem / non-K8s mode),
        // Selector falls back to Same: same-ns allowed, cross-ns denied.
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Selector".into()),
                selector: Some(serde_json::json!({ "matchLabels": { "env": "prod" } })),
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "gw-ns", "gw-ns"));
        assert!(!listener_allows_route_namespace(&allowed, "other-ns", "gw-ns"));
    }

    #[test]
    fn test_ns_policy_selector_with_store() {
        use crate::core::controller::conf_mgr::sync_runtime::resource_processor::namespace_store::get_namespace_store;
        use std::collections::BTreeMap;

        let store = get_namespace_store();
        store.upsert(k8s_openapi::api::core::v1::Namespace {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some("allowed-ns".into()),
                labels: Some(BTreeMap::from([("env".into(), "prod".into())])),
                ..Default::default()
            },
            ..Default::default()
        });
        store.upsert(k8s_openapi::api::core::v1::Namespace {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some("denied-ns".into()),
                labels: Some(BTreeMap::from([("env".into(), "dev".into())])),
                ..Default::default()
            },
            ..Default::default()
        });

        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Selector".into()),
                selector: Some(serde_json::json!({ "matchLabels": { "env": "prod" } })),
            }),
            kinds: None,
        });

        assert!(listener_allows_route_namespace(&allowed, "allowed-ns", "gw-ns"));
        assert!(!listener_allows_route_namespace(&allowed, "denied-ns", "gw-ns"));
        // unknown-ns: labels not in store → falls back to Same → denied (different ns)
        assert!(!listener_allows_route_namespace(&allowed, "unknown-ns", "gw-ns"));

        store.remove("allowed-ns");
        store.remove("denied-ns");
    }
}

/// Register Service backend references for a route.
///
/// Tracks which Services this route depends on, so that when a Service
/// changes, the route is requeued to re-evaluate its ResolvedRefs status.
pub fn register_service_backend_refs(
    route_kind: crate::types::ResourceKind,
    route_ns: &str,
    route_name: &str,
    backend_refs: &[(Option<&str>, Option<&str>, &str)], // (kind, namespace, name)
) {
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::service_ref::get_service_ref_manager;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let resource_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());

    let manager = get_service_ref_manager();
    manager.clear_resource_refs(&resource_ref);

    for &(kind, namespace, name) in backend_refs {
        let kind_str = kind.unwrap_or("Service");
        if kind_str != "Service" {
            continue;
        }
        let backend_ns = namespace.unwrap_or(route_ns);
        let service_key = format!("{}/{}", backend_ns, name);
        manager.add_ref(service_key, resource_ref.clone());
    }
}

/// Clear Service backend references for a deleted route.
pub fn clear_service_backend_refs(route_kind: crate::types::ResourceKind, route_ns: &str, route_name: &str) {
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::service_ref::get_service_ref_manager;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let resource_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());
    get_service_ref_manager().clear_resource_refs(&resource_ref);
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

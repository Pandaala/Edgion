//! Shared utilities for Route and Gateway handlers
//!
//! Deduplicates common logic used across gateway.rs, http_route.rs,
//! grpc_route.rs, and other route handlers.

use std::collections::HashSet;
use std::sync::Arc;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::attached_route_tracker::Attachment;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{AcceptedError, ResolvedRefsError};
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::types::listener_key::{resolve_listener_addr, ListenerKey};
use crate::types::prelude_resources::Gateway;
use crate::types::resources::common::{ParentReference, RefDenied};
use crate::types::resources::gateway::AllowedRoutes;
use crate::types::resources::http_route::RouteParentStatus;
use crate::types::ResourceKind;

/// Result of validating a parentRef for acceptance.
pub struct AcceptedResult {
    /// Acceptance errors. Non-empty means the route is rejected.
    pub errors: Vec<AcceptedError>,
    /// Names of listeners that accept this route (namespace + hostname intersection pass).
    /// Empty when the route is rejected.
    pub matched_listeners: Vec<String>,
    /// Protocols of the matched listeners (e.g. "HTTPS", "TLS", "HTTP").
    /// Populated alongside matched_listeners; same order is NOT guaranteed.
    pub matched_listener_protocols: Vec<String>,
}

/// Check if a listener's namespace policy allows a route from the given namespace.
///
/// Per Gateway API spec, the default (no AllowedRoutes or no namespaces field)
/// is "Same" — only routes from the same namespace as the Gateway are allowed.
pub fn listener_allows_route_namespace(
    allowed_routes: &Option<AllowedRoutes>,
    route_ns: &str,
    gateway_ns: &str,
    runtime: &ControllerRuntime,
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
            let store = &runtime.namespace_store;
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
pub fn service_exists(namespace: &str, name: &str, runtime: &ControllerRuntime) -> bool {
    let Some(processor) = runtime.processor_registry.get("Service") else {
        return true;
    };
    processor.contains_key(&format!("{}/{}", namespace, name))
}

/// Validate backend refs: check group/kind target core Service and the Service exists.
///
/// Each tuple is `(kind, namespace, name)` extracted from the route's backend refs.
/// Returns typed errors for ResolvedRefs condition generation.
pub fn validate_backend_refs(
    route_ns: &str,
    backend_refs: &[(Option<&str>, Option<&str>, Option<&str>, &str)],
    runtime: &ControllerRuntime,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for &(group, kind, namespace, name) in backend_refs {
        let group_str = group.unwrap_or("");
        if !group_str.is_empty() && group_str != "core" {
            errors.push(ResolvedRefsError::InvalidKind {
                kind: format!("{group_str}/{}", kind.unwrap_or("Service")),
                name: name.to_string(),
            });
            continue;
        }
        let kind_str = kind.unwrap_or("Service");
        if kind_str != "Service" {
            errors.push(ResolvedRefsError::InvalidKind {
                kind: kind_str.to_string(),
                name: name.to_string(),
            });
            continue;
        }
        let backend_ns = namespace.unwrap_or(route_ns);
        if !service_exists(backend_ns, name, runtime) {
            errors.push(ResolvedRefsError::BackendNotFound {
                namespace: backend_ns.to_string(),
                name: name.to_string(),
            });
        }
    }
    errors
}

/// Collect RefNotPermitted errors from ref_denied fields on backend refs.
///
/// `from_ns` / `from_kind` identify the referring Route (e.g. `HTTPRoute` in
/// `app-ns`). They are folded into `ResolvedRefsError::RefNotPermitted` so the
/// Condition `message` stays self-describing once it leaves the CRD's
/// `metadata` context (events, log aggregation, dashboards).
pub fn collect_ref_denied_errors(
    from_ns: &str,
    from_kind: ResourceKind,
    ref_denied_list: &[Option<&RefDenied>],
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for ref_denied in ref_denied_list.iter().flatten() {
        errors.push(ResolvedRefsError::RefNotPermitted {
            from_namespace: from_ns.to_string(),
            from_kind: from_kind.as_str(),
            target_namespace: ref_denied.target_namespace.clone(),
            target_name: ref_denied.target_name.clone(),
        });
    }
    errors
}

/// Gateway API standard route kinds — only these are subject to allowedRoutes.kinds filtering.
/// Non-route resources (e.g. EdgionTls for TLS config) bypass this check entirely.
const GATEWAY_API_ROUTE_KINDS: &[&str] = &["HTTPRoute", "GRPCRoute", "TLSRoute", "TCPRoute", "UDPRoute"];

/// Protocol default route kinds per Gateway API spec.
/// Mirrors `get_supported_kinds_for_protocol()` in gateway.rs — keep in sync.
pub(super) fn protocol_default_route_kinds(protocol: &str) -> &'static [&'static str] {
    match protocol.to_uppercase().as_str() {
        "HTTP" | "HTTPS" => &["HTTPRoute", "GRPCRoute"],
        "TLS" => &["TLSRoute"],
        "TCP" => &["TCPRoute"],
        "UDP" => &["UDPRoute"],
        _ => &[],
    }
}

/// Check if a listener supports the given route kind based on allowedRoutes.kinds
/// or protocol defaults per Gateway API spec.
///
/// Non-route resources (e.g. EdgionTls) always pass — `allowedRoutes.kinds` only
/// governs Gateway API route kinds, not policy/config attachments.
fn listener_supports_route_kind(listener: &crate::types::resources::gateway::Listener, route_kind: &str) -> bool {
    if !GATEWAY_API_ROUTE_KINDS.contains(&route_kind) {
        return true;
    }

    let kinds = listener
        .allowed_routes
        .as_ref()
        .and_then(|ar| ar.kinds.as_ref())
        .filter(|k| !k.is_empty());

    match kinds {
        Some(allowed_kinds) => allowed_kinds.iter().any(|rgk| {
            let group = rgk.group.as_deref().unwrap_or("gateway.networking.k8s.io");
            group == "gateway.networking.k8s.io" && rgk.kind == route_kind
        }),
        None => protocol_default_route_kinds(&listener.protocol).contains(&route_kind),
    }
}

/// Validate a parent ref for the Accepted condition.
///
/// Checks namespace policy and (optionally) hostname intersection.
/// For L4 routes without hostnames, pass `None` to skip hostname checks.
pub fn validate_parent_ref_accepted(
    route_ns: &str,
    parent_ref: &ParentReference,
    route_hostnames: Option<&Vec<String>>,
    route_kind: &str,
    runtime: &ControllerRuntime,
) -> AcceptedResult {
    let parent_group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
    if parent_group != "gateway.networking.k8s.io" {
        return AcceptedResult {
            errors: Vec::new(),
            matched_listeners: Vec::new(),
            matched_listener_protocols: Vec::new(),
        };
    }
    let parent_kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
    if parent_kind != "Gateway" {
        return AcceptedResult {
            errors: Vec::new(),
            matched_listeners: Vec::new(),
            matched_listener_protocols: Vec::new(),
        };
    }

    let parent_ns = parent_ref.namespace.as_deref().unwrap_or(route_ns);
    let parent_name = &parent_ref.name;

    let Some(gateway) = lookup_gateway(parent_ns, parent_name, runtime) else {
        return AcceptedResult {
            errors: Vec::new(),
            matched_listeners: Vec::new(),
            matched_listener_protocols: Vec::new(),
        };
    };

    let empty_listeners = Vec::new();
    let listeners = gateway.spec.listeners.as_ref().unwrap_or(&empty_listeners);

    if let Some(section_name) = &parent_ref.section_name {
        let has_listener = listeners
            .iter()
            .any(|l| l.name == *section_name && parent_ref.port.is_none_or(|p| l.port == p));
        if !has_listener {
            return AcceptedResult {
                errors: vec![AcceptedError::NoMatchingParent {
                    section_name: section_name.clone(),
                }],
                matched_listeners: Vec::new(),
                matched_listener_protocols: Vec::new(),
            };
        }
    } else if let Some(port) = parent_ref.port {
        let has_listener = listeners.iter().any(|l| l.port == port);
        if !has_listener {
            return AcceptedResult {
                errors: vec![AcceptedError::NoMatchingParent {
                    section_name: format!("port:{}", port),
                }],
                matched_listeners: Vec::new(),
                matched_listener_protocols: Vec::new(),
            };
        }
    }

    let matching_listeners: Vec<_> = listeners
        .iter()
        .filter(|l| {
            parent_ref.section_name.as_ref().is_none_or(|sn| l.name == *sn)
                && parent_ref.port.is_none_or(|p| l.port == p)
        })
        .collect();

    let ns_hostname_accepted: Vec<_> = matching_listeners
        .iter()
        .filter(|l| listener_allows_route_namespace(&l.allowed_routes, route_ns, parent_ns, runtime))
        .filter(|l| match route_hostnames {
            None => true,
            Some(route_hs) if route_hs.is_empty() => true,
            Some(route_hs) => match &l.hostname {
                None => true,
                Some(listener_hn) => route_hs
                    .iter()
                    .any(|route_hn| hostnames_intersect(listener_hn, route_hn)),
            },
        })
        .collect();

    let accepted: Vec<_> = ns_hostname_accepted
        .iter()
        .filter(|l| listener_supports_route_kind(l, route_kind))
        .collect();
    let accepted_listener_names: Vec<String> = accepted.iter().map(|l| l.name.clone()).collect();
    let accepted_listener_protocols: Vec<String> = accepted.iter().map(|l| l.protocol.clone()).collect();

    if accepted_listener_names.is_empty() {
        let ns_allowed = matching_listeners
            .iter()
            .any(|l| listener_allows_route_namespace(&l.allowed_routes, route_ns, parent_ns, runtime));
        let errors = if !ns_allowed {
            vec![AcceptedError::NotAllowedByListeners {
                resource_ns: route_ns.to_string(),
            }]
        } else if ns_hostname_accepted.is_empty() {
            vec![AcceptedError::NoMatchingListenerHostname {
                hostnames: route_hostnames.cloned().unwrap_or_default(),
            }]
        } else {
            vec![AcceptedError::NotAllowedByListenerKind {
                resource_kind: route_kind.to_string(),
            }]
        };
        return AcceptedResult {
            errors,
            matched_listeners: Vec::new(),
            matched_listener_protocols: Vec::new(),
        };
    }

    AcceptedResult {
        errors: Vec::new(),
        matched_listeners: accepted_listener_names,
        matched_listener_protocols: accepted_listener_protocols,
    }
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

/// Update the attached-route tracker with only accepted parentRefs.
///
/// Called from `update_status()` in each route handler, after acceptance has
/// been computed. Only parentRefs that pass all acceptance checks (namespace,
/// sectionName/port match, hostname intersection) are counted.
pub fn update_tracker_accepted_only(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    parent_refs: Option<&Vec<ParentReference>>,
    route_hostnames: Option<&Vec<String>>,
    runtime: &ControllerRuntime,
) -> bool {
    let route_key = format!("{}/{}", route_ns, route_name);
    let Some(parent_refs) = parent_refs else {
        return runtime.attached_route_tracker.remove_route(route_kind, &route_key);
    };

    let mut attachments = HashSet::new();

    for parent_ref in parent_refs {
        let parent_group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
        let parent_kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
        if parent_group != "gateway.networking.k8s.io" || parent_kind != "Gateway" {
            continue;
        }

        let result = validate_parent_ref_accepted(route_ns, parent_ref, route_hostnames, route_kind.as_str(), runtime);
        if !result.errors.is_empty() {
            continue;
        }

        let gateway_ns = parent_ref.namespace.as_deref().unwrap_or(route_ns);
        let gateway_key = format!("{}/{}", gateway_ns, parent_ref.name);
        for listener_name in &result.matched_listeners {
            attachments.insert(Attachment {
                gateway_key: gateway_key.clone(),
                listener_name: listener_name.clone(),
            });
        }
    }

    runtime
        .attached_route_tracker
        .update_route(route_kind, &route_key, attachments)
}

/// Register Service backend references for a route.
///
/// Tracks which Services this route depends on, so that when a Service
/// changes, the route is requeued to re-evaluate its ResolvedRefs status.
pub fn register_service_backend_refs(
    route_kind: crate::types::ResourceKind,
    route_ns: &str,
    route_name: &str,
    backend_refs: &[(Option<&str>, Option<&str>, Option<&str>, &str)], // (group, kind, namespace, name)
    runtime: &ControllerRuntime,
) {
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let resource_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());

    let manager = &runtime.service_ref_mgr;
    manager.clear_resource_refs(&resource_ref);

    for &(group, kind, namespace, name) in backend_refs {
        let group_str = group.unwrap_or("");
        if !group_str.is_empty() && group_str != "core" {
            continue;
        }
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
pub fn clear_service_backend_refs(
    route_kind: crate::types::ResourceKind,
    route_ns: &str,
    route_name: &str,
    runtime: &ControllerRuntime,
) {
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let resource_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());
    runtime.service_ref_mgr.clear_resource_refs(&resource_ref);
}

/// Look up a Gateway resource from the processor registry.
///
/// Uses the raw store (`get_raw_any`) so that Gateways not belonging to
/// this controller's `gatewayClassName` (which are excluded from `ServerCache`
/// by the handler filter) are still found. This fixes incorrect Route status
/// calculation when a Route's parentRef points to a foreign Gateway.
pub fn lookup_gateway(namespace: &str, name: &str, runtime: &ControllerRuntime) -> Option<Arc<Gateway>> {
    let processor = runtime.processor_registry.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_raw_any(&key)?.downcast::<Gateway>().ok()
}

/// Resolve listener keys from parentRefs for per-listener route isolation.
///
/// For each parentRef, looks up the referenced Gateway and resolves matching
/// listeners based on the parentRef's sectionName and/or port fields:
///
/// - **sectionName set**: find the listener by name; if port is also set,
///   validate that the listener's port matches (per Gateway API spec, both
///   must match when both are specified).
/// - **port set (no sectionName)**: find all listeners matching the port.
/// - **neither set**: attach to all listeners of the parent Gateway.
///
/// Only includes listeners whose `allowedRoutes` namespace policy permits
/// the route's namespace (default policy: "Same").
///
/// `route_namespace` is the namespace of the route resource itself.
/// `fallback_namespace` is typically `route.metadata.namespace` and is used
/// to resolve the Gateway namespace when `parentRef.namespace` is absent.
pub fn resolve_listener_keys(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    fallback_namespace: Option<&str>,
    runtime: &ControllerRuntime,
) -> Vec<ListenerKey> {
    let (mut keys, _) = resolve_listeners_inner(parent_refs, route_namespace, fallback_namespace, runtime);
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Like `resolve_listener_keys`, but also returns the listener hostnames
/// (positionally paired with the keys) for effective hostname computation.
pub fn resolve_listener_keys_with_hostnames(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    fallback_namespace: Option<&str>,
    runtime: &ControllerRuntime,
) -> (Vec<ListenerKey>, Vec<Option<String>>) {
    resolve_listeners_inner(parent_refs, route_namespace, fallback_namespace, runtime)
}

fn resolve_listeners_inner(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    fallback_namespace: Option<&str>,
    runtime: &ControllerRuntime,
) -> (Vec<ListenerKey>, Vec<Option<String>>) {
    let mut listener_keys = Vec::new();
    let mut listener_hostnames: Vec<Option<String>> = Vec::new();

    for parent_ref in parent_refs {
        let gw_ns = parent_ref
            .namespace
            .as_deref()
            .or(fallback_namespace)
            .unwrap_or("default");

        let Some(gateway) = lookup_gateway(gw_ns, &parent_ref.name, runtime) else {
            // Parent Gateway not in the store at compute time (e.g. the route
            // was processed before its parent Gateway). The route gets no
            // resolved listeners for this parentRef until the Gateway is
            // processed and requeues the route.
            tracing::info!(
                component = "resolve_listener_keys",
                gateway = %format!("{}/{}", gw_ns, parent_ref.name),
                "parentRef parent Gateway not found in store; route gets no resolved listeners for this parentRef"
            );
            continue;
        };
        let gw_addresses = gateway.spec.addresses.as_ref();
        let Some(listeners) = &gateway.spec.listeners else {
            tracing::info!(
                component = "resolve_listener_keys",
                gateway = %format!("{}/{}", gw_ns, parent_ref.name),
                "parentRef parent Gateway found but has no listeners; route gets no resolved listeners for this parentRef"
            );
            continue;
        };
        tracing::debug!(
            component = "resolve_listener_keys",
            gateway = %format!("{}/{}", gw_ns, parent_ref.name),
            listeners = ?listeners.iter().map(|l| (l.name.clone(), l.port)).collect::<Vec<_>>(),
            section_name = ?parent_ref.section_name,
            parent_port = ?parent_ref.port,
            "parentRef parent Gateway found; resolving matching listeners"
        );

        let mut collect = |l: &crate::types::resources::gateway::Listener| {
            if listener_allows_route_namespace(&l.allowed_routes, route_namespace, gw_ns, runtime) {
                let addr = resolve_listener_addr(gw_addresses, l.port);
                listener_keys.push(ListenerKey::new(addr, &l.protocol));
                listener_hostnames.push(l.hostname.clone());
            }
        };

        if let Some(section_name) = &parent_ref.section_name {
            if let Some(listener) = listeners.iter().find(|l| l.name == *section_name) {
                if let Some(port) = parent_ref.port {
                    if listener.port != port {
                        continue;
                    }
                }
                collect(listener);
            }
        } else if let Some(port) = parent_ref.port {
            for listener in listeners {
                if listener.port == port {
                    collect(listener);
                }
            }
        } else {
            for listener in listeners {
                collect(listener);
            }
        }
    }

    (listener_keys, listener_hostnames)
}

/// Compute effective hostnames = intersection of route hostnames with listener hostnames.
///
/// Per Gateway API spec, a route's effective hostname on a listener is the
/// intersection of the route's hostnames with the listener's hostname.
/// The result is deduplicated.
pub fn compute_effective_hostnames(route_hostnames: &[String], listener_hostnames: &[Option<String>]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut effective: BTreeSet<String> = BTreeSet::new();

    if route_hostnames.is_empty() {
        // Route matches all hostnames; effective = listener hostnames
        for lh in listener_hostnames {
            if let Some(h) = lh {
                effective.insert(h.to_ascii_lowercase());
            }
            // None listener hostname = all hostnames; nothing specific to add
        }
        return effective.into_iter().collect();
    }

    for lh in listener_hostnames {
        match lh {
            None => {
                // Listener accepts all hostnames; effective = route hostnames
                for rh in route_hostnames {
                    effective.insert(rh.to_ascii_lowercase());
                }
            }
            Some(listener_host) => {
                for rh in route_hostnames {
                    if let Some(intersected) = hostname_intersection(listener_host, rh) {
                        effective.insert(intersected.to_ascii_lowercase());
                    }
                }
            }
        }
    }

    effective.into_iter().collect()
}

/// Compute the intersection of two hostnames, returning the more specific one.
/// Returns None if they do not intersect.
fn hostname_intersection<'a>(a: &'a str, b: &'a str) -> Option<&'a str> {
    if hostnames_intersect(a, b) {
        // Return the more specific hostname (non-wildcard > wildcard, longer suffix > shorter)
        let a_wild = a.starts_with("*.");
        let b_wild = b.starts_with("*.");
        match (a_wild, b_wild) {
            (false, true) => Some(a), // a is exact, b is wildcard
            (true, false) => Some(b), // b is exact, a is wildcard
            _ => {
                // Both exact (must be equal) or both wildcard (more specific = longer)
                if a.len() >= b.len() {
                    Some(a)
                } else {
                    Some(b)
                }
            }
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::gateway::{AllowedRoutes, RouteNamespaces};

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

    #[test]
    fn test_ns_policy_none_defaults_to_same() {
        let runtime = crate::core::controller::conf_mgr::ControllerRuntime::default();
        assert!(listener_allows_route_namespace(&None, "ns1", "ns1", &runtime));
        assert!(!listener_allows_route_namespace(&None, "ns1", "ns2", &runtime));
    }

    #[test]
    fn test_ns_policy_all() {
        let runtime = crate::core::controller::conf_mgr::ControllerRuntime::default();
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("All".into()),
                selector: None,
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "any-ns", "gw-ns", &runtime));
    }

    #[test]
    fn test_ns_policy_same() {
        let runtime = crate::core::controller::conf_mgr::ControllerRuntime::default();
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Same".into()),
                selector: None,
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "ns1", "ns1", &runtime));
        assert!(!listener_allows_route_namespace(&allowed, "ns1", "ns2", &runtime));
    }

    #[test]
    fn test_ns_policy_selector_no_selector_field() {
        let runtime = crate::core::controller::conf_mgr::ControllerRuntime::default();
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Selector".into()),
                selector: None,
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "any-ns", "gw-ns", &runtime));
    }

    #[test]
    fn test_ns_policy_selector_falls_back_to_same_when_labels_missing() {
        let runtime = crate::core::controller::conf_mgr::ControllerRuntime::default();
        let allowed = Some(AllowedRoutes {
            namespaces: Some(RouteNamespaces {
                from: Some("Selector".into()),
                selector: Some(serde_json::json!({ "matchLabels": { "env": "prod" } })),
            }),
            kinds: None,
        });
        assert!(listener_allows_route_namespace(&allowed, "gw-ns", "gw-ns", &runtime));
        assert!(!listener_allows_route_namespace(
            &allowed, "other-ns", "gw-ns", &runtime
        ));
    }

    #[test]
    fn test_ns_policy_selector_with_store() {
        use std::collections::BTreeMap;

        let runtime = crate::core::controller::conf_mgr::ControllerRuntime::default();
        runtime.namespace_store.upsert(k8s_openapi::api::core::v1::Namespace {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some("allowed-ns".into()),
                labels: Some(BTreeMap::from([("env".into(), "prod".into())])),
                ..Default::default()
            },
            ..Default::default()
        });
        runtime.namespace_store.upsert(k8s_openapi::api::core::v1::Namespace {
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

        assert!(listener_allows_route_namespace(
            &allowed,
            "allowed-ns",
            "gw-ns",
            &runtime
        ));
        assert!(!listener_allows_route_namespace(
            &allowed,
            "denied-ns",
            "gw-ns",
            &runtime
        ));
        assert!(!listener_allows_route_namespace(
            &allowed,
            "unknown-ns",
            "gw-ns",
            &runtime
        ));

        runtime.namespace_store.remove("allowed-ns");
        runtime.namespace_store.remove("denied-ns");
    }
}

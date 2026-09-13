//! Shared utilities for Route and Gateway handlers
//!
//! Deduplicates common logic used across gateway.rs, http_route.rs,
//! grpc_route.rs, and other route handlers.

use std::collections::HashSet;
use std::sync::Arc;

use crate::conf_mgr::sync_runtime::resource_processor::attached_route_tracker::Attachment;
use crate::conf_mgr::sync_runtime::resource_processor::{AcceptedError, ResolvedRefsError};
use crate::conf_mgr::ControllerRuntime;
use edgion_algo::matcher::radix_hostname::RadixHostnamePatternRef;
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::prelude_resources::{EdgionPlugins, Gateway};
use edgion_resources::resources::common::{ParentReference, RefDenied};
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use edgion_resources::resources::extension_ref::is_supported_edgion_plugins_ref;
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::resources::http_route::{LocalObjectReference, RouteParentStatus};
use edgion_resources::ResourceKind;

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

/// Return the validation error for an EdgionPlugins backend filter whose
/// desired resource declares a plugin unavailable at backend scope.
///
/// Backend filters execute only after backend selection, which is too late to
/// participate in the Global/Gateway/Route WAF ownership election. The raw
/// desired generation is intentional: EdgionPlugins requeues dependents before
/// its processed cache is replaced, so the cache may still contain the prior
/// generation during this route reconcile.
pub(super) fn backend_request_scope_extension_ref_error(
    runtime: &ControllerRuntime,
    route_namespace: &str,
    backend_index: usize,
    extension_ref: &LocalObjectReference,
) -> Option<String> {
    if !is_supported_edgion_plugins_ref(extension_ref) {
        return None;
    }
    let namespace = extension_ref.resolved_namespace.as_deref().unwrap_or(route_namespace);
    let processor = runtime.processor_registry.get("EdgionPlugins")?;
    let key = format!("{namespace}/{}", extension_ref.name);
    let plugins = processor
        .get_raw_any(&key)
        .and_then(|raw| raw.downcast::<EdgionPlugins>().ok())?;
    let plugin = if plugins.spec.waf.is_some() {
        "WAF"
    } else if plugins.has_active_request_id() {
        "RequestId"
    } else {
        return None;
    };
    Some(format!(
        "backendRefs[{backend_index}] EdgionPlugins ExtensionRef cannot contain {plugin}; attach {plugin} at Global, Gateway, or Route scope"
    ))
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
    hostname_intersection(listener_hn, route_hn).is_some()
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
fn listener_supports_route_kind(listener: &edgion_resources::resources::gateway::Listener, route_kind: &str) -> bool {
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
            errors: vec![AcceptedError::ParentNotFound {
                parent_name: format!("{}/{}", parent_ns, parent_name),
            }],
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
///
/// Returns the Service keys the route no longer references (dropped by this
/// update). The caller MUST requeue backend policies (BackendTLSPolicy and
/// EdgionBackendTrafficPolicy) bound to those Services via
/// `requeue_backend_policies_for_services`: a coalesced delete->recreate
/// skips `on_delete`'s requeue for the previous generation, and even a plain
/// update that drops a backend Service would otherwise never notify that
/// Service's policies to re-resolve their ancestor Gateways.
pub fn register_service_backend_refs(
    route_kind: edgion_resources::ResourceKind,
    route_ns: &str,
    route_name: &str,
    backend_refs: &[(Option<&str>, Option<&str>, Option<&str>, &str)], // (group, kind, namespace, name)
    runtime: &ControllerRuntime,
) -> Vec<String> {
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let resource_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());

    let manager = &runtime.service_ref_mgr;

    // Snapshot the Services this route referenced before this reconcile so the
    // caller can requeue BTPs bound to any Service dropped from backendRefs.
    let old_service_keys: HashSet<String> = manager.get_dependencies(&resource_ref.key()).into_iter().collect();

    manager.clear_resource_refs(&resource_ref);

    let mut new_service_keys = HashSet::new();
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
        manager.add_ref(service_key.clone(), resource_ref.clone());
        new_service_keys.insert(service_key);
    }

    old_service_keys.difference(&new_service_keys).cloned().collect()
}

/// Clear Service backend references for a deleted route.
pub fn clear_service_backend_refs(
    route_kind: edgion_resources::ResourceKind,
    route_ns: &str,
    route_name: &str,
    runtime: &ControllerRuntime,
) {
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let resource_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());
    runtime.service_ref_mgr.clear_resource_refs(&resource_ref);
}

/// Look up a Gateway resource from the processor registry.
///
/// Uses the raw store (`get_raw_any`) so that Gateways not belonging to
/// this controller's `gatewayClassName` (which are excluded from `ServerCache`
/// by the handler filter) are still found. Without this, Routes with a
/// parentRef to a foreign Gateway would receive incorrect status.
pub fn lookup_gateway(namespace: &str, name: &str, runtime: &ControllerRuntime) -> Option<Arc<Gateway>> {
    let processor = runtime.processor_registry.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_raw_any(&key)?.downcast::<Gateway>().ok()
}

fn lookup_projected_gateway(namespace: &str, name: &str, runtime: &ControllerRuntime) -> Option<Arc<Gateway>> {
    let processor = runtime.processor_registry.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_cached_any(&key)?.downcast::<Gateway>().ok()
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
    let (mut keys, _, _) =
        resolve_listeners_inner(parent_refs, route_namespace, fallback_namespace, runtime, None, false);
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Resolve TLSRoute hostnames independently for each exact listener attachment.
pub fn resolve_tls_listener_hostnames(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    hostnames: &[String],
    runtime: &ControllerRuntime,
) -> Vec<edgion_resources::resources::tls_route::ResolvedTlsListener> {
    use edgion_resources::resources::tls_route::ResolvedTlsListener;
    if hostnames.is_empty() {
        return Vec::new();
    }
    let (_, listener_hosts, attachments) =
        resolve_listeners_inner(parent_refs, route_namespace, Some(route_namespace), runtime, None, true);
    let mut resolved = Vec::new();
    for (attachment, listener_host) in attachments.into_iter().zip(listener_hosts) {
        if attachment.listener_key.protocol() != "TLS" {
            continue;
        }
        let effective = compute_tls_hostnames(hostnames, listener_host.as_deref());
        if effective.is_empty() {
            continue;
        }
        resolved.push(ResolvedTlsListener {
            listener_key: attachment.listener_key,
            gateway_namespace: attachment.gateway_namespace,
            gateway_name: attachment.gateway_name,
            listener_name: attachment.listener_name,
            hostnames: effective,
        });
    }
    resolved.sort_by(|a, b| {
        (&a.listener_key, &a.gateway_namespace, &a.gateway_name, &a.listener_name).cmp(&(
            &b.listener_key,
            &b.gateway_namespace,
            &b.gateway_name,
            &b.listener_name,
        ))
    });
    resolved.dedup();
    resolved
}

/// Resolve complete Gateway listener attachment identities for EdgionTls.
pub fn resolve_listener_attachments(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    fallback_namespace: Option<&str>,
    runtime: &ControllerRuntime,
) -> Vec<ResolvedListenerAttachment> {
    let (_, _, mut attachments) = resolve_listeners_inner(
        parent_refs,
        route_namespace,
        fallback_namespace,
        runtime,
        Some(edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_RESOLVED_ATTACHMENTS),
        false,
    );
    // Raw-store Gateways intentionally carry no controller instance proof.
    // They remain usable for ordinary route status/listener-key resolution,
    // but must never mint an EdgionTls overlay attachment. The Gateway
    // on-change path requeues EdgionTls after the projected object is cached.
    attachments.retain(|attachment| attachment.gateway_proof.is_resolved());
    attachments.sort_by(|a, b| {
        (&a.gateway_namespace, &a.gateway_name, &a.listener_name, &a.listener_key).cmp(&(
            &b.gateway_namespace,
            &b.gateway_name,
            &b.listener_name,
            &b.listener_key,
        ))
    });
    attachments.dedup();
    attachments
}

fn resolve_listeners_inner(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    fallback_namespace: Option<&str>,
    runtime: &ControllerRuntime,
    attachment_limit: Option<usize>,
    tls_route: bool,
) -> (Vec<ListenerKey>, Vec<Option<String>>, Vec<ResolvedListenerAttachment>) {
    let mut listener_keys = Vec::new();
    let mut listener_hostnames: Vec<Option<String>> = Vec::new();
    let mut listener_attachments = Vec::new();

    for parent_ref in parent_refs {
        // Only Gateway parentRefs bind to listeners. Delegation parentRefs
        // (`kind: HTTPRoute` reverse locks) and other kinds are skipped.
        let parent_group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
        let parent_kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
        if parent_group != "gateway.networking.k8s.io" || parent_kind != "Gateway" {
            continue;
        }

        let gw_ns = parent_ref
            .namespace
            .as_deref()
            .or(fallback_namespace)
            .unwrap_or("default");

        let Some(gateway) = lookup_projected_gateway(gw_ns, &parent_ref.name, runtime)
            .or_else(|| lookup_gateway(gw_ns, &parent_ref.name, runtime))
        else {
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
        if listeners.len() > edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS {
            tracing::warn!(
                component = "resolve_listener_keys",
                gateway = %format!("{}/{}", gw_ns, parent_ref.name),
                listener_count = listeners.len(),
                max = edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS,
                "Ignoring Gateway whose listener count exceeds the runtime limit"
            );
            continue;
        }
        tracing::debug!(
            component = "resolve_listener_keys",
            gateway = %format!("{}/{}", gw_ns, parent_ref.name),
            listeners = ?listeners.iter().map(|l| (l.name.clone(), l.port)).collect::<Vec<_>>(),
            section_name = ?parent_ref.section_name,
            parent_port = ?parent_ref.port,
            "parentRef parent Gateway found; resolving matching listeners"
        );

        let mut collect = |l: &edgion_resources::resources::gateway::Listener| {
            if tls_route && (l.protocol != "TLS" || !listener_supports_route_kind(l, "TLSRoute")) {
                return;
            }
            if attachment_limit.is_some_and(|limit| listener_attachments.len() > limit) {
                return;
            }
            if listener_allows_route_namespace(&l.allowed_routes, route_namespace, gw_ns, runtime) {
                let addr = resolve_listener_addr(gw_addresses, l.port);
                let listener_key = ListenerKey::new(addr, &l.protocol);
                listener_keys.push(listener_key.clone());
                listener_hostnames.push(l.hostname.clone());
                listener_attachments.push(ResolvedListenerAttachment {
                    gateway_namespace: gw_ns.to_string(),
                    gateway_name: parent_ref.name.clone(),
                    listener_name: l.name.clone(),
                    listener_key,
                    gateway_proof: edgion_resources::resources::gateway::gateway_attachment_proof(&gateway),
                });
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

    (listener_keys, listener_hostnames, listener_attachments)
}

/// Project one TLS attachment; an unrestricted listener preserves the route names.
fn compute_tls_hostnames(route_hostnames: &[String], listener_hostname: Option<&str>) -> Vec<String> {
    route_hostnames
        .iter()
        .filter_map(|hostname| match listener_hostname {
            Some(listener) => hostname_intersection(listener, hostname),
            None => Some(hostname.as_str()),
        })
        .map(str::to_ascii_lowercase)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Compute the intersection of two hostnames, returning the more specific one.
/// Returns None if they do not intersect.
fn hostname_intersection<'a>(a: &'a str, b: &'a str) -> Option<&'a str> {
    let a = RadixHostnamePatternRef::parse(a).ok()?;
    let b = RadixHostnamePatternRef::parse(b).ok()?;
    a.intersection(b).map(RadixHostnamePatternRef::as_str)
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_patterns_never_intersect() {
        for invalid in ["*", "A.example.com", "127.0.0.1", "a..b"] {
            assert!(!super::hostnames_intersect(invalid, invalid));
        }
        assert!(super::hostnames_intersect("a", "a"));
        assert!(super::hostnames_intersect("*.127.0.0.1", "a.127.0.0.1"));
    }

    #[tokio::test]
    async fn tls_hostnames_remain_scoped_to_attachments_through_updates() {
        use super::*;
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            16,
            Arc::new(GatewayHandler::new(None, None)),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        processor.set_raw_store(raw.clone());
        runtime.processor_registry.register(processor);
        let gateway = |name: &str, port: u16, hostname: Option<&str>| {
            Arc::new(
                serde_json::from_value::<Gateway>(serde_json::json!({
                    "metadata": {"namespace": "apps", "name": name},
                    "spec": {"gatewayClassName": "test", "listeners": [{
                        "name": "tls", "port": port, "protocol": "TLS", "hostname": hostname,
                        "tls": {"mode": "Passthrough"}
                    }]}
                }))
                .unwrap(),
            )
        };
        raw.upsert("apps/a".into(), gateway("a", 8443, Some("a.example.com")));
        raw.upsert("apps/b".into(), gateway("b", 9443, Some("b.example.com")));
        let parents: Vec<ParentReference> = serde_json::from_value(serde_json::json!([
            {"name": "a", "sectionName": "tls"}, {"name": "b", "sectionName": "tls"}
        ]))
        .unwrap();
        let hosts = vec!["*.example.com".to_string()];
        let resolved = resolve_tls_listener_hostnames(&parents, "apps", &hosts, &runtime);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].hostnames, ["a.example.com"]);
        assert_eq!(resolved[1].hostnames, ["b.example.com"]);
        assert_eq!(resolved[0].gateway_name, "a");
        assert_eq!(resolved[1].listener_key.0, "0.0.0.0:9443/TLS");
        // An unrestricted listener preserves the explicit route wildcard; it
        // does not widen the other attachment or manufacture a route catch-all.
        raw.upsert("apps/a".into(), gateway("a", 8443, None));
        let updated = resolve_tls_listener_hostnames(&parents, "apps", &hosts, &runtime);
        assert_eq!(updated[0].hostnames, ["*.example.com"]);
        assert_eq!(updated[1], resolved[1]);
        raw.upsert("apps/a".into(), gateway("a", 10443, Some("unrelated.test")));
        let narrowed = resolve_tls_listener_hostnames(&parents, "apps", &hosts, &runtime);
        assert_eq!(narrowed, [resolved[1].clone()]);
        assert!(resolve_tls_listener_hostnames(&parents, "apps", &[], &runtime).is_empty());
        let port_mismatch: Vec<ParentReference> = serde_json::from_value(serde_json::json!([
            {"name": "b", "sectionName": "tls", "port": 8443}
        ]))
        .unwrap();
        assert!(resolve_tls_listener_hostnames(&port_mismatch, "apps", &hosts, &runtime).is_empty());
        let mut mixed = (*gateway("a", 8443, Some("a.example.com"))).clone();
        mixed.spec.listeners.as_mut().unwrap().push(
            serde_json::from_value(serde_json::json!({
                "name": "wrong-group", "port": 8443, "protocol": "TLS", "hostname": "a.example.com",
                "allowedRoutes": {"kinds": [{"kind": "TLSRoute", "group": "unrelated.example.com"}]}
            }))
            .unwrap(),
        );
        raw.upsert("apps/a".into(), Arc::new(mixed));
        let all: Vec<ParentReference> = serde_json::from_value(serde_json::json!([{"name": "a"}])).unwrap();
        let filtered = resolve_tls_listener_hostnames(&all, "apps", &hosts, &runtime);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].listener_name, "tls");
    }

    #[tokio::test]
    async fn backend_request_id_rejects_only_active_entries() {
        use super::*;
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        processor.set_raw_store(raw.clone());
        runtime.processor_registry.register(processor);
        let reference: LocalObjectReference = serde_json::from_value(serde_json::json!({
            "group": "edgion.io", "kind": "EdgionPlugins", "name": "ids"
        }))
        .unwrap();
        for enabled in [true, false, true] {
            let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
                "metadata": {"namespace": "apps", "name": "ids"},
                "spec": {"requestPlugins": [{"type": "RequestId", "enable": enabled, "config": {}}]}
            }))
            .unwrap();
            raw.upsert("apps/ids".to_string(), Arc::new(plugins));
            let error = backend_request_scope_extension_ref_error(&runtime, "apps", 2, &reference);
            assert_eq!(error.is_some(), enabled);
            if let Some(error) = error {
                assert!(error.contains("backendRefs[2]"));
                assert!(error.contains("cannot contain RequestId"));
            }
        }
    }

    use super::*;
    use edgion_resources::resources::gateway::{AllowedRoutes, RouteNamespaces};

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
        let runtime = crate::conf_mgr::ControllerRuntime::default();
        assert!(listener_allows_route_namespace(&None, "ns1", "ns1", &runtime));
        assert!(!listener_allows_route_namespace(&None, "ns1", "ns2", &runtime));
    }

    #[test]
    fn test_resolve_listener_keys_skips_delegation_parent_refs() {
        // A `kind: HTTPRoute` parentRef (delegation reverse lock) must not be
        // looked up as a Gateway — it yields no listener keys and no side
        // effects (previously it would log a bogus "Gateway not found").
        let runtime = crate::conf_mgr::ControllerRuntime::default();
        let delegation_ref = ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("HTTPRoute".to_string()),
            namespace: Some("infra".to_string()),
            name: "parent-route".to_string(),
            section_name: None,
            port: None,
        };
        let keys = resolve_listener_keys(&[delegation_ref], "billing", Some("billing"), &runtime);
        assert!(keys.is_empty());
    }

    #[test]
    fn test_ns_policy_all() {
        let runtime = crate::conf_mgr::ControllerRuntime::default();
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
        let runtime = crate::conf_mgr::ControllerRuntime::default();
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
        let runtime = crate::conf_mgr::ControllerRuntime::default();
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
        let runtime = crate::conf_mgr::ControllerRuntime::default();
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

        let runtime = crate::conf_mgr::ControllerRuntime::default();
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

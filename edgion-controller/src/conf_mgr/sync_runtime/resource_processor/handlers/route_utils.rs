//! Shared utilities for Route and Gateway handlers
//!
//! Deduplicates common logic used across gateway.rs, http_route.rs,
//! grpc_route.rs, and other route handlers.

use edgion_resources::resources::refs::plugin_target::{classify, PluginTarget};
use std::collections::HashSet;
use std::sync::Arc;

use crate::conf_mgr::sync_runtime::resource_processor::attached_route_tracker::Attachment;
use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::{parent_is_owned, serving_gateway};
use crate::conf_mgr::sync_runtime::resource_processor::{AcceptedError, ResolvedRefsError, ResourceRef};
use crate::conf_mgr::ControllerRuntime;
use edgion_algo::matcher::radix_hostname::RadixHostnamePatternRef;
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::prelude_resources::{EdgionPlugins, Gateway};
use edgion_resources::resources::common::{ParentReference, RefDenied};
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::resources::http_route::RouteParentStatus;
use edgion_resources::resources::refs::route_fields::{RouteExtensionField, RouteExtensionLookup, RouteFilterSlot};
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

/// Failure to resolve the single Gateway attachment used by an EdgionAcme
/// HTTP-01 challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcmeGatewayRefError {
    InvalidKind { group: String, kind: String },
    RefNotPermitted { namespace: String, name: String },
    ParentNotFound { namespace: String, name: String },
    NoMatchingListener,
    NoHttpListener,
    TooManyListeners,
    UnresolvedGateway,
}

/// Resolve an EdgionAcme `gatewayRef` to Controller-owned HTTP listeners.
///
/// Unlike Route attachment resolution, this deliberately ignores
/// `allowedRoutes`, Route kinds, and Route hostname intersection. EdgionAcme is
/// not a Route. Only the projected Gateway cache is authoritative here: raw
/// Gateways have no resolved attachment proof and cannot mint serving rights.
pub(crate) fn resolve_acme_listener_attachments(
    parent_ref: &ParentReference,
    owner_namespace: &str,
    runtime: &ControllerRuntime,
) -> Result<Vec<ResolvedListenerAttachment>, AcmeGatewayRefError> {
    let group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
    let kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
    if group != "gateway.networking.k8s.io" || kind != "Gateway" {
        return Err(AcmeGatewayRefError::InvalidKind {
            group: group.to_string(),
            kind: kind.to_string(),
        });
    }

    let gateway_namespace = parent_ref.namespace.as_deref().unwrap_or(owner_namespace);
    if gateway_namespace != owner_namespace
        && crate::config::is_reference_grant_validation_enabled()
        && !runtime.ref_grant_store.check_reference_allowed(
            owner_namespace,
            "edgion.io",
            "EdgionAcme",
            gateway_namespace,
            "gateway.networking.k8s.io",
            "Gateway",
            Some(&parent_ref.name),
        )
    {
        return Err(AcmeGatewayRefError::RefNotPermitted {
            namespace: gateway_namespace.to_string(),
            name: parent_ref.name.clone(),
        });
    }

    let gateway = serving_gateway(gateway_namespace, &parent_ref.name, runtime).ok_or_else(|| {
        AcmeGatewayRefError::ParentNotFound {
            namespace: gateway_namespace.to_string(),
            name: parent_ref.name.clone(),
        }
    })?;
    let proof = edgion_resources::resources::gateway::gateway_attachment_proof(&gateway);
    if !proof.is_resolved() {
        return Err(AcmeGatewayRefError::UnresolvedGateway);
    }

    let listeners = gateway.spec.listeners.as_deref().unwrap_or_default();
    if listeners.len() > edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS {
        return Err(AcmeGatewayRefError::TooManyListeners);
    }

    let selected: Vec<_> = listeners
        .iter()
        .filter(|listener| {
            parent_ref
                .section_name
                .as_ref()
                .is_none_or(|section_name| listener.name == *section_name)
                && parent_ref.port.is_none_or(|port| listener.port == port)
                && !edgion_resources::resources::gateway::gateway_listener_is_explicitly_ineligible(
                    &gateway,
                    &listener.name,
                )
        })
        .collect();
    if selected.is_empty() {
        return Err(AcmeGatewayRefError::NoMatchingListener);
    }

    let mut attachments: Vec<_> = selected
        .into_iter()
        .filter(|listener| listener.protocol == "HTTP")
        .map(|listener| ResolvedListenerAttachment {
            gateway_namespace: gateway_namespace.to_string(),
            gateway_name: parent_ref.name.clone(),
            listener_name: listener.name.clone(),
            listener_key: ListenerKey::new(resolve_listener_addr(&gateway, listener.port), &listener.protocol),
            gateway_proof: proof.clone(),
        })
        .collect();
    if attachments.is_empty() {
        return Err(AcmeGatewayRefError::NoHttpListener);
    }
    if attachments.len() > edgion_resources::resources::edgion_acme::MAX_ACME_RESOLVED_ATTACHMENTS {
        return Err(AcmeGatewayRefError::TooManyListeners);
    }
    attachments.sort_by(|a, b| {
        (&a.gateway_namespace, &a.gateway_name, &a.listener_name, &a.listener_key).cmp(&(
            &b.gateway_namespace,
            &b.gateway_name,
            &b.listener_name,
            &b.listener_key,
        ))
    });
    attachments.dedup();
    Ok(attachments)
}

/// A dependency receipt retains the declaration that produced its valid target.
/// It cannot authorize a lookup or mint a ReferenceGrant query.
#[derive(Clone, Debug)]
pub(super) struct ExtensionDependencyReceipt {
    key: String,
    _source: ResourceRef,
    _source_uid: Option<String>,
    _source_generation: Option<i64>,
    _slot: RouteFilterSlot,
}
impl ExtensionDependencyReceipt {
    pub(super) fn from_field(field: RouteExtensionField<'_>) -> Option<Self> {
        let reference = field.validated().ok()??;
        Some(Self::from_lookup(reference))
    }
    fn from_lookup(reference: RouteExtensionLookup<'_>) -> Self {
        let target = reference.target();
        let source = reference.source();
        Self {
            key: format!(
                "{}/{}",
                target
                    .scope()
                    .namespace()
                    .expect("ExtensionRef targets are local and namespaced"),
                target.name()
            ),
            _source: ResourceRef::new(
                reference.source_kind(),
                source.scope().namespace().map(str::to_string),
                source.name().to_string(),
            ),
            _source_uid: reference.source_uid().map(str::to_string),
            _source_generation: reference.source_generation(),
            _slot: reference.slot(),
        }
    }
    pub(super) fn register(&self, root: &ResourceRef, runtime: &ControllerRuntime) {
        runtime.plugin_ref_mgr.add_ref(self.key.clone(), root.clone());
    }
}

/// Inspect semantic targets without changing raw source ownership or authorization.
pub(super) fn plugin_target(runtime: &ControllerRuntime, key: &str) -> PluginTarget<Arc<EdgionPlugins>> {
    classify(key, |raw_key| {
        runtime
            .processor_registry
            .get("EdgionPlugins")?
            .get_raw_any(raw_key)?
            .downcast::<EdgionPlugins>()
            .ok()
    })
    .unwrap_or(PluginTarget::Missing)
}

/// Collision is diagnostic only: retain the matched rule and selected backend.
pub(super) fn ambiguous_plugin_ref(
    field: RouteExtensionField<'_>,
    runtime: &ControllerRuntime,
) -> Option<ResolvedRefsError> {
    let reference = field.validated().ok()??;
    let target = reference.target();
    let namespace = target.scope().namespace()?;
    let key = format!("{namespace}/{}", target.name());
    matches!(plugin_target(runtime, &key), PluginTarget::Ambiguous).then(|| ResolvedRefsError::AmbiguousEdgionPlugins {
        namespace: namespace.to_owned(),
        name: target.name().to_owned(),
    })
}

/// Register before consulting the desired plugin generation. The root owns the
/// wakeup; only the actual field controls the target's namespace and identity.
fn extension_scope_violation_with(
    field: RouteExtensionField<'_>,
    inspect: bool,
    register: impl FnOnce(&ExtensionDependencyReceipt),
    read: impl FnMut(&str) -> Option<Arc<EdgionPlugins>>,
) -> Option<(&'static str, bool)> {
    let reference = field.validated().ok()??;
    let receipt = ExtensionDependencyReceipt::from_lookup(reference);
    register(&receipt);
    let backend = reference.slot().backend().is_some();
    if !inspect || (!backend && reference.source_kind() != ResourceKind::GRPCRoute) {
        return None;
    }
    let PluginTarget::Unique { source: plugins, .. } = classify(&receipt.key, read).ok()? else {
        return None;
    };
    if !backend {
        return plugins
            .has_active_grpc_transcoder()
            .then_some(("GrpcJsonTranscoder", true));
    }
    if plugins.spec.waf.is_some() {
        Some(("WAF", true))
    } else if plugins.has_active_grpc_transcoder() {
        Some(("GrpcJsonTranscoder (HTTPRoute rule only)", true))
    } else if plugins.has_active_request_id() {
        Some(("RequestId", true))
    } else if let Some(entry) = plugins
        .spec
        .request_plugins
        .iter()
        .flatten()
        .find(|entry| entry.is_enabled() && entry.plugin.publishes_upstream_jump())
    {
        Some((entry.plugin.type_name(), false))
    } else {
        None
    }
}

fn extension_scope_violation(
    runtime: &ControllerRuntime,
    root: &ResourceRef,
    field: RouteExtensionField<'_>,
    inspect: bool,
) -> Option<(&'static str, bool)> {
    extension_scope_violation_with(
        field,
        inspect,
        |receipt| receipt.register(root, runtime),
        |key| {
            runtime
                .processor_registry
                .get("EdgionPlugins")?
                .get_raw_any(key)?
                .downcast::<EdgionPlugins>()
                .ok()
        },
    )
}

fn scope_error(field: RouteExtensionField<'_>, plugin: &'static str) -> String {
    match field.slot().backend() {
        Some(index) if plugin.starts_with("GrpcJsonTranscoder") =>
            format!("backendRefs[{index}] GrpcJsonTranscoder requires HTTPRoute rule scope"),
        Some(index) => format!("backendRefs[{index}] EdgionPlugins ExtensionRef cannot contain {plugin}; attach {plugin} at Global, Gateway, or Route scope"),
        None => "GrpcJsonTranscoder requires HTTPRoute rule scope".into(),
    }
}

/// Delegation always registers valid declarations before drops. Structurally
/// ineligible rules register recovery edges without reading plugin content.
pub(super) fn delegated_extension_ref_error(
    runtime: &ControllerRuntime,
    root: &ResourceRef,
    field: RouteExtensionField<'_>,
    inspect: bool,
) -> Option<String> {
    extension_scope_violation(runtime, root, field, inspect).map(|(plugin, _)| scope_error(field, plugin))
}

pub(super) fn backend_request_scope_extension_ref_error(
    runtime: &ControllerRuntime,
    root: &ResourceRef,
    field: RouteExtensionField<'_>,
) -> Option<String> {
    let (plugin, _) = extension_scope_violation(runtime, root, field, true)?;
    Some(scope_error(field, plugin))
}

/// WAF, RequestId and transcoder retain their route-wide authored rejection;
/// a late upstream jump only invalidates the containing rule.
pub(super) fn backend_request_scope_route_wide_error(
    runtime: &ControllerRuntime,
    root: &ResourceRef,
    field: RouteExtensionField<'_>,
) -> Option<String> {
    let (plugin, route_wide) = extension_scope_violation(runtime, root, field, true)?;
    route_wide.then(|| scope_error(field, plugin))
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

const GATEWAY_API_GROUP: &str = "gateway.networking.k8s.io";

/// Protocol default route kinds per Gateway API spec.
pub(super) fn protocol_default_route_kinds(protocol: &str) -> &'static [&'static str] {
    match protocol.to_uppercase().as_str() {
        "HTTP" | "HTTPS" => &["HTTPRoute", "GRPCRoute"],
        "TLS" => &["TLSRoute"],
        "TCP" => &["TCPRoute"],
        "UDP" => &["UDPRoute"],
        _ => &[],
    }
}

/// Return whether a Route group/kind is compatible with a Listener protocol.
///
/// This is the canonical compatibility decision for Listener `supportedKinds`,
/// Route acceptance, and attachment projection. `allowedRoutes.kinds` may
/// narrow this set, but it cannot make an incompatible kind valid.
pub(super) fn protocol_supports_route_kind(protocol: &str, group: &str, route_kind: &str) -> bool {
    group == GATEWAY_API_GROUP && protocol_default_route_kinds(protocol).contains(&route_kind)
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

    if !protocol_supports_route_kind(&listener.protocol, GATEWAY_API_GROUP, route_kind) {
        return false;
    }

    let kinds = listener
        .allowed_routes
        .as_ref()
        .and_then(|ar| ar.kinds.as_ref())
        .filter(|k| !k.is_empty());

    match kinds {
        Some(allowed_kinds) => allowed_kinds.iter().any(|rgk| {
            let group = rgk.group.as_deref().unwrap_or(GATEWAY_API_GROUP);
            group == GATEWAY_API_GROUP && rgk.kind == route_kind
        }),
        None => true,
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

/// Reuse only this controller's exact attachment, retaining condition transition times.
pub(super) fn parent_status_for_update<'a>(
    parents: &'a mut Vec<RouteParentStatus>,
    parent_ref: &ParentReference,
    resource_namespace: &str,
    controller_name: &str,
) -> &'a mut RouteParentStatus {
    let identity = parent_ref.canonical_identity(Some(resource_namespace));
    if let Some(index) = parents.iter().position(|status| {
        status.controller_name == controller_name
            && status.parent_ref.canonical_identity(Some(resource_namespace)) == identity
    }) {
        return &mut parents[index];
    }
    parents.push(RouteParentStatus {
        parent_ref: parent_ref.clone(),
        controller_name: controller_name.to_owned(),
        conditions: Vec::new(),
    });
    parents.last_mut().expect("a parent status was just inserted")
}

/// Remove stale or duplicate owned statuses, preserving foreign input entries verbatim.
/// The first owned entry is the one updated by `parent_status_for_update`.
pub(super) fn retain_current_parent_statuses(
    parents: &mut Vec<RouteParentStatus>,
    parent_refs: &[ParentReference],
    resource_namespace: &str,
    controller_name: &str,
) {
    let previous = std::mem::take(parents);
    parents.reserve(previous.len());
    for status in previous {
        if status.controller_name != controller_name {
            parents.push(status);
            continue;
        }
        let identity = status.canonical_identity(Some(resource_namespace));
        let current = parent_refs
            .iter()
            .any(|parent| parent.canonical_identity(Some(resource_namespace)) == identity.parent);
        let duplicate = parents
            .iter()
            .any(|retained| retained.canonical_identity(Some(resource_namespace)) == identity);
        if current && !duplicate {
            parents.push(status);
        }
    }
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

        if !parent_is_owned(parent_ref, route_ns, runtime) {
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
pub(super) fn register_service_backend_refs(
    resource_ref: &crate::conf_mgr::sync_runtime::resource_processor::ResourceRef,
    backend_refs: &[super::route_backend_refs::BackendDependencyReceipt],
    runtime: &ControllerRuntime,
) -> Vec<String> {
    let old = begin_service_refs(resource_ref, runtime);
    finish_service_refs(resource_ref, backend_refs, old, runtime)
}

pub(super) fn begin_service_refs(
    resource_ref: &crate::conf_mgr::sync_runtime::resource_processor::ResourceRef,
    runtime: &ControllerRuntime,
) -> HashSet<String> {
    let manager = &runtime.service_ref_mgr;
    let old = crate::conf_mgr::sync_runtime::resource_processor::service_ref::forwarding_service_dependencies(
        manager,
        &resource_ref.key(),
    )
    .into_iter()
    .collect();
    manager.clear_value_refs(resource_ref);
    old
}

pub(super) fn finish_service_refs(
    resource_ref: &crate::conf_mgr::sync_runtime::resource_processor::ResourceRef,
    backend_refs: &[super::route_backend_refs::BackendDependencyReceipt],
    old: HashSet<String>,
    runtime: &ControllerRuntime,
) -> Vec<String> {
    let mut current = HashSet::new();
    for receipt in backend_refs {
        if let Some(key) = receipt.service_key() {
            runtime.service_ref_mgr.add_ref(key.to_string(), resource_ref.clone());
            current.insert(key.to_string());
        }
    }
    old.difference(&current).cloned().collect()
}

/// Clear Service backend references for a deleted route.
pub fn clear_service_backend_refs(
    resource_ref: &crate::conf_mgr::sync_runtime::resource_processor::ResourceRef,
    runtime: &ControllerRuntime,
) {
    runtime.service_ref_mgr.clear_value_refs(resource_ref);
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

/// Preserve Gateway ownership alongside the local listener key for TCP/UDP
/// route projection into a dedicated data-plane process.
pub fn resolve_route_listener_attachments(
    parent_refs: &[ParentReference],
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Vec<edgion_resources::resources::common::RouteListenerAttachment> {
    use edgion_resources::resources::common::RouteListenerAttachment;
    let (_, _, attachments) = resolve_listeners_inner(
        parent_refs,
        route_namespace,
        Some(route_namespace),
        runtime,
        None,
        false,
    );
    let mut result: Vec<RouteListenerAttachment> = attachments
        .into_iter()
        .map(|attachment| RouteListenerAttachment {
            gateway_namespace: attachment.gateway_namespace,
            gateway_name: attachment.gateway_name,
            listener_name: attachment.listener_name,
            listener_key: attachment.listener_key,
        })
        .collect();
    result.sort();
    result.dedup();
    result
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

        let Some(gateway) = serving_gateway(gw_ns, &parent_ref.name, runtime).or_else(|| {
            runtime
                .status_identity()
                .is_none()
                .then(|| lookup_gateway(gw_ns, &parent_ref.name, runtime))
                .flatten()
        }) else {
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
                let addr = resolve_listener_addr(&gateway, l.port);
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_utils_test.rs"]
mod tests;

/// Route-kind validation uses the same raw source generation as dependencies.
pub(super) fn contains_grpc_transcoder(
    runtime: &ControllerRuntime,
    root: &ResourceRef,
    field: RouteExtensionField<'_>,
) -> bool {
    extension_scope_violation(runtime, root, field, true)
        .is_some_and(|(plugin, _)| plugin.starts_with("GrpcJsonTranscoder"))
}

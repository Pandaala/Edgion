//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use edgion_resources::resources::refs::route_fields::{HttpRouteMirrorRefs, MirrorSlot, TerminalMirrorReference};
use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::validate_http_route_if_enabled;
use super::route_backend_refs::{self, BackendDependencyReceipt};
use super::route_delegation::{flatten_route, populate_terminal_rule_identities, DelegationLookup};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_policies_for_backend_targets,
    requeue_backend_policies_for_services, requeue_parent_gateways, update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, set_parent_conditions_full, update_condition,
    AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::{EdgionBackend, HTTPRoute};
use edgion_resources::resources::common::{ParentReference, RefDenied};
#[cfg(test)]
use edgion_resources::resources::edgion_plugins::{AiProxyConfig, EdgionPlugin, EdgionPlugins};
use edgion_resources::resources::http_route::{
    DelegationIssue, HTTPRouteFilterType, HTTPRouteRule, HTTPRouteStatus, GATEWAY_API_GROUP,
};
#[cfg(test)]
use edgion_resources::resources::http_route_ai_admission::{self as shared_ai_admission, AiProxyAdmission};
use edgion_resources::resources::http_route_ai_admission::{ResolvedAiAdmission, ResolvedForwardingTarget};
use edgion_resources::resources::refs::extension_ref::{
    is_supported_edgion_plugins_ref, validate_extension_ref_filter, ExtensionRefValidationError,
};
use edgion_resources::resources::refs::route_fields::{
    EffectiveBackendReference, HttpRouteBackendRefs, HttpRouteExtensionRefs, RouteExtensionField,
    TerminalBackendReference,
};
use edgion_resources::resources::refs::targets::BackendTarget;
use edgion_resources::ResourceKind;

/// True for parentRef entries that attach the route to a Gateway listener
/// (the Gateway API default when group/kind are absent). Delegation
/// parentRefs (`kind: HTTPRoute`, reverse-lock declarations) return false.
fn is_gateway_parent_ref(pr: &ParentReference) -> bool {
    pr.canonical_identity(None).is_gateway()
}

/// True for parentRef entries that reverse-lock delegation to specific parent
/// HTTPRoutes.
fn is_delegation_parent_ref(pr: &ParentReference) -> bool {
    let identity = pr.canonical_identity(None);
    identity.group == GATEWAY_API_GROUP && identity.kind == "HTTPRoute"
}

/// `DelegationLookup` backed by the live controller stores.
struct RuntimeDelegationLookup<'a> {
    runtime: &'a ControllerRuntime,
    grant_owner: Option<ResourceRef>,
}

impl DelegationLookup for RuntimeDelegationLookup<'_> {
    fn get_route(&self, key: &str) -> Option<Arc<HTTPRoute>> {
        let processor = self.runtime.processor_registry.get("HTTPRoute")?;
        // Visibility must match the wildcard listing (ServerCache): the raw
        // store bypasses the controller's namespace/metadata filters, so a
        // named delegation to a filtered-out child would otherwise expand a
        // route the controller never processes (no on_change → stale forever,
        // and a de-facto namespace-filter bypass).
        if !processor.contains_key(key) {
            return None;
        }
        processor.get_raw_any(key)?.downcast::<HTTPRoute>().ok()
    }

    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String> {
        let Some(processor) = self.runtime.processor_registry.get("HTTPRoute") else {
            return Vec::new();
        };
        let prefix = format!("{ns}/");
        let mut keys: Vec<String> = processor
            .list_keys()
            .into_iter()
            .filter(|k| k.starts_with(&prefix))
            .collect();
        keys.sort_unstable();
        keys
    }

    fn mirror_ref_denied(&self, terminal: TerminalMirrorReference<'_>) -> Option<RefDenied> {
        self.grant_owner
            .as_ref()
            .and_then(|root| super::request_mirror::authorize(terminal, root, true, self.runtime))
    }
    fn terminal_ref_denied(&self, terminal: TerminalBackendReference<'_>) -> Option<RefDenied> {
        self.grant_owner
            .as_ref()
            .and_then(|owner| route_backend_refs::authorize_terminal(terminal, owner, self.runtime))
    }
    fn extension_ref_error(&self, field: RouteExtensionField<'_>, inspect: bool) -> Option<String> {
        self.grant_owner
            .as_ref()
            .and_then(|root| super::route_utils::delegated_extension_ref_error(self.runtime, root, field, inspect))
    }
}

/// HTTPRoute handler
pub struct HttpRouteHandler {
    controller_name: String,
}

impl HttpRouteHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }

    fn create_resource_ref(route: &HTTPRoute) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::HTTPRoute,
            route.metadata.namespace.clone(),
            route.metadata.name.clone().unwrap_or_default(),
        )
    }

    /// Register Service backend references for cross-resource requeue.
    ///
    /// `extra_dependencies` carries the flattened delegation backendRefs (with
    /// explicit namespaces) so Service changes under delegated children
    /// requeue this parent route directly. Parse already cleared the old set
    /// before mirror authorization. Add forwarding edges without clearing the
    /// valid child mirror edges retained through delegation and rule drops.
    fn register_service_refs(
        route: &HTTPRoute,
        extra_dependencies: &[BackendDependencyReceipt],
        ctx: &HandlerContext,
        old: HashSet<String>,
    ) -> Vec<String> {
        let mut backend_refs_list = HttpRouteBackendRefs::from_route(route)
            .map(|references| route_backend_refs::collect_dependencies(references.authored()))
            .unwrap_or_default();
        backend_refs_list.extend_from_slice(extra_dependencies);

        super::route_utils::finish_service_refs(
            &Self::create_resource_ref(route),
            &backend_refs_list,
            old,
            ctx.runtime(),
        )
    }

    /// Record cross-namespace references from backend_refs.
    ///
    /// `extra_namespaces` carries the cross-namespace targets of flattened
    /// delegation backendRefs so ReferenceGrant changes in those namespaces
    /// requeue this parent route. Extend the set cleared at parse entry without
    /// removing dependencies registered before mirror authorization checks.
    fn record_cross_ns_refs(route: &HTTPRoute, extra_namespaces: &HashSet<String>, ctx: &HandlerContext) {
        if HttpRouteBackendRefs::from_route(route).is_err() {
            return;
        }
        let owner = Self::create_resource_ref(route);
        for namespace in extra_namespaces {
            ctx.runtime().cross_ns_ref_mgr.add_ref(namespace.clone(), owner.clone());
        }
    }

    /// Register all EdgionPlugins ExtensionRefs for plugin-change requeue.
    ///
    /// BackendRef filters participate because a referenced generation that
    /// adds or removes WAF changes this route's ownership validity. Replacing
    /// the full ref set on every reconcile also handles delete-to-create
    /// coalescing.
    fn register_plugin_refs(route: &HTTPRoute, ctx: &HandlerContext) {
        let Ok(references) = HttpRouteExtensionRefs::from_route(route) else {
            return;
        };
        let root = Self::create_resource_ref(route);
        // Parse entry clears the old generation. These additive passes retain
        // valid dependencies from authored, inherited, and later-dropped rules.
        for field in references.authored().chain(references.effective()) {
            if let Some(receipt) = super::route_utils::ExtensionDependencyReceipt::from_field(field) {
                receipt.register(&root, ctx.runtime());
            }
        }
    }

    /// Replace this route's EdgionBackend dependency set and return the
    /// old/new union plus the removed subset. The union is needed because an
    /// EBTP attached to either side may change the route's Gateway ancestors.
    fn register_backend_refs(
        route: &HTTPRoute,
        flattened_dependencies: &[BackendDependencyReceipt],
        ctx: &HandlerContext,
    ) -> BackendRefDelta {
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().backend_ref_mgr;
        let old: HashSet<String> = manager.clear_value_refs(&resource_ref).into_iter().collect();
        let mut receipts = HttpRouteBackendRefs::from_route(route)
            .map(|references| route_backend_refs::collect_dependencies(references.authored()))
            .unwrap_or_default();
        receipts.extend_from_slice(flattened_dependencies);
        let new: HashSet<String> = receipts
            .iter()
            .filter_map(|receipt| receipt.backend_key().map(str::to_string))
            .collect();
        for key in &new {
            manager.add_ref(key.clone(), resource_ref.clone());
        }
        let mut affected: Vec<String> = old.union(&new).cloned().collect();
        affected.sort_unstable();
        let mut dropped: Vec<String> = old.difference(&new).cloned().collect();
        dropped.sort_unstable();
        BackendRefDelta { affected, dropped }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct BackendRefDelta {
    affected: Vec<String>,
    dropped: Vec<String>,
}

impl Default for HttpRouteHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<HTTPRoute> for HttpRouteHandler {
    fn should_publish(&self, route: &HTTPRoute, _ctx: &HandlerContext) -> bool {
        route.spec.unsupported_default_gateway_attachment().is_none()
    }

    fn finalize_transport(&self, route: &mut HTTPRoute, _ctx: &HandlerContext) {
        route.spec.resolved_status_controller = Some(self.controller_name.clone());
    }

    fn validate(&self, route: &HTTPRoute, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = validate_http_route_if_enabled(route);
        if let Some(reason) = route.spec.unsupported_default_gateway_attachment() {
            errors.push(reason.to_string());
        }
        // Note: per-rule validation (filter conflicts, malformed CORS) is handled by
        // find_invalid_rule_details() in update_status(), which marks individual rules
        // as invalid via PartiallyInvalid condition rather than rejecting the entire route.
        if let Err(error) = HttpRouteBackendRefs::from_route(route) {
            errors.push(route_backend_refs::source_error_message(error));
        }
        errors
    }

    async fn parse(&self, mut route: HTTPRoute, _ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        let source_valid = HttpRouteBackendRefs::from_route(&route).is_ok();
        let grant_owner = Self::create_resource_ref(&route);
        _ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&grant_owner);
        _ctx.runtime().plugin_ref_mgr.clear_value_refs(&grant_owner);
        let old_services = super::route_utils::begin_service_refs(&grant_owner, _ctx.runtime());
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let route_ns = route_ns.as_str();

        if route.spec.unsupported_default_gateway_attachment().is_some() {
            // The only publication decision for this source is a full reject.
            // Avoid registering dependencies for an object that will never
            // enter the cache; an earlier published generation is removed by
            // the processor's should_publish=false transition.
            route.spec.resolved_rules = None;
            route.spec.resolved_listeners = None;
            route.spec.resolved_hostnames = None;
            requeue_backend_policies_for_services(&old_services.iter().cloned().collect::<Vec<_>>(), _ctx).await;
            return ProcessResult::Continue(route);
        }

        // This is controller-owned transport metadata. Kubernetes prunes it
        // via schemars, while filesystem/etcd inputs need the same explicit
        // defense before delegated copies are stamped with their declaring
        // namespace.
        for rule in route.spec.rules.iter_mut().flatten() {
            rule.resolved_ai_admission = None;
            for filter in rule.filters.iter_mut().flatten().chain(
                rule.backend_refs
                    .iter_mut()
                    .flatten()
                    .flat_map(|backend_ref| backend_ref.filters.iter_mut().flatten()),
            ) {
                if filter.filter_type == HTTPRouteFilterType::ExtensionRef {
                    if let Some(extension_ref) = filter.extension_ref.as_mut() {
                        extension_ref.resolved_namespace = None;
                    }
                }
            }
        }

        // Terminal identity is controller-owned transport metadata. Stamp the
        // root rules before flattening so non-delegating siblings inherit it;
        // delegated child terminals are stamped by the flattener itself.
        populate_terminal_rule_identities(&mut route);

        let mirror_decisions = HttpRouteMirrorRefs::from_route(&route)
            .map(|refs| {
                refs.authored()
                    .map(|field| {
                        super::request_mirror::resolve(field, |terminal| {
                            super::request_mirror::authorize(terminal, &grant_owner, true, _ctx.runtime())
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for decision in mirror_decisions {
            if let MirrorSlot::Route(slot) = decision.slot() {
                if let Some(rule) = route.spec.rules.as_mut().and_then(|rules| rules.get_mut(slot.rule())) {
                    decision.apply_rule(rule);
                }
            }
        }
        // Authorize actual authored loans before cloning any effective rules.
        let decisions = HttpRouteBackendRefs::from_route(&route)
            .map(|references| route_backend_refs::resolve_denials(references.authored(), &grant_owner, _ctx.runtime()))
            .unwrap_or_default();
        for rule in route.spec.rules.iter_mut().flatten() {
            for backend in rule.backend_refs.iter_mut().flatten() {
                backend.ref_denied = None;
            }
        }
        for (slot, denial) in decisions {
            if let Some(backend) = route
                .spec
                .rules
                .as_mut()
                .and_then(|rules| rules.get_mut(slot.rule()))
                .and_then(|rule| rule.backend_refs.as_mut())
                .and_then(|backends| backends.get_mut(slot.backend()))
            {
                backend.ref_denied = denial;
            }
        }

        // HTTPRoute delegation: expand delegation trees into resolved_rules.
        // Runs early so the flattened Service/cross-ns refs land in the single
        // registration pass below. resolved_rules/delegation_issues are
        // controller-derived: clear any user-provided or stale values.
        route.spec.resolved_rules = None;
        route.spec.delegation_issues = Vec::new();
        Self::register_plugin_refs(&route, _ctx);
        let mut delegation_outcome = {
            let adapter = RuntimeDelegationLookup {
                runtime: _ctx.runtime(),
                grant_owner: Some(grant_owner.clone()),
            };
            flatten_route(&route, &adapter)
        };
        // Dependency registration below must inspect the rules that the
        // Gateway will actually compile, including delegated child filters.
        route.spec.resolved_rules = delegation_outcome.resolved_rules.clone();

        // Register before protocol filtering so a rule removed below retains
        // the dependency that can make it eligible again on a later reconcile.
        Self::register_plugin_refs(&route, _ctx);
        for receipt in &delegation_outcome.extension_dependencies {
            receipt.register(&grant_owner, _ctx.runtime());
        }

        let reasons: Vec<_> = route
            .spec
            .resolved_rules
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, _)| backend_plugin_scope_invalid_reason(&route, index, true, _ctx.runtime(), false))
            .collect();
        if let Some(rules) = &mut route.spec.resolved_rules {
            let mut reasons = reasons.into_iter().enumerate();
            rules.retain(|_| {
                let (resolved_index, reason) = reasons.next().expect("one decision per borrowed resolved rule");
                let keep = reason.is_none();
                if let Some(reason) = reason {
                    delegation_outcome.issues.push(DelegationIssue::RuleDropped {
                        message: format!("resolvedRules[{resolved_index}]: {reason}"),
                    });
                }
                keep
            });
            delegation_outcome.resolved_rules = route.spec.resolved_rules.clone();
        }

        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route, &delegation_outcome.cross_ns_target_namespaces, _ctx);

        let backend_ref_delta = Self::register_backend_refs(&route, &delegation_outcome.backend_dependencies, _ctx);
        requeue_backend_policies_for_backend_targets(&backend_ref_delta.affected, _ctx).await;
        tracing::debug!(
            route = %format!("{route_ns}/{}", route.metadata.name.as_deref().unwrap_or("")),
            dropped_backends = ?backend_ref_delta.dropped,
            "Replaced HTTPRoute EdgionBackend references"
        );

        // Register Service backend references for cross-resource requeue, then
        // requeue BTPs bound to any Service dropped from backendRefs (the apply
        // path otherwise only requeues BTPs for the route's current backends).
        let dropped_service_keys =
            Self::register_service_refs(&route, &delegation_outcome.backend_dependencies, _ctx, old_services);
        requeue_backend_policies_for_services(&dropped_service_keys, _ctx).await;

        // Register dependencies before reading backend source facts. Recompute
        // flattened slots after delegation; never trust a child's old sidecar.
        populate_ai_admission(&mut route, _ctx.runtime());

        // Maintain the delegation index (also clears stale edges when the
        // route no longer delegates) and requeue children whose delegation
        // edges changed so their child-side status stays fresh.
        {
            let route_key = format!("{route_ns}/{}", route.metadata.name.as_deref().unwrap_or(""));
            let delta = _ctx.runtime().route_delegation_index.update_parent(
                &route_key,
                delegation_outcome.named_children.clone(),
                delegation_outcome.wildcard_namespaces.clone(),
            );
            for child_key in &delta.changed_named {
                if *child_key != route_key {
                    _ctx.requeue("HTTPRoute", child_key.clone()).await;
                }
            }
            if !delta.changed_namespaces.is_empty() {
                let adapter = RuntimeDelegationLookup {
                    runtime: _ctx.runtime(),
                    grant_owner: None,
                };
                for ns in &delta.changed_namespaces {
                    for key in adapter.list_route_keys_in_namespace(ns) {
                        if key != route_key {
                            _ctx.requeue("HTTPRoute", key).await;
                        }
                    }
                }
            }

            // Close the flatten↔delete race: a child deleted between the
            // flatten read and the update_parent registration above would
            // neither be in resolved_rules' future (its on_delete ran too
            // early to see our edge) nor trigger a re-flatten. Verify the
            // expanded children still exist and self-requeue on mismatch
            // (TriggerChain caps bound any pathological repetition).
            {
                let adapter = RuntimeDelegationLookup {
                    runtime: _ctx.runtime(),
                    grant_owner: None,
                };
                let vanished = delegation_outcome
                    .expanded_children
                    .iter()
                    .any(|key| adapter.get_route(key).is_none());
                if vanished {
                    _ctx.requeue("HTTPRoute", route_key.clone()).await;
                }
            }
        }
        route.spec.delegation_issues = delegation_outcome.issues;

        // Treat resolved_listeners as controller-derived state.
        // Clear any user-provided or stale value before recomputing from current parentRefs.
        route.spec.resolved_listeners = None;
        route.spec.resolved_hostnames = None;
        if let Some(annotations) = route.metadata.annotations.as_mut() {
            annotations.remove("edgion.io/hostname-resolution");
        }
        let valid_hostnames = source_valid && route.spec.validate_hostnames().is_ok();

        // Resolve effective hostnames (intersection with Gateway listener hostnames)
        if let Some(parent_refs) = route.spec.parent_refs.as_ref().filter(|_| valid_hostnames) {
            let route_ns_resolve = route.metadata.namespace.as_deref().unwrap_or("default");
            let resolved = super::hostname_resolution::resolve_effective_hostnames(
                route.spec.hostnames.as_ref(),
                parent_refs,
                route_ns_resolve,
                _ctx.runtime(),
            );
            route.spec.resolved_hostnames = if resolved.hostnames.is_empty() {
                None
            } else {
                Some(resolved.hostnames)
            };
            if let Some(annotation) = resolved.annotation {
                let annotations = route.metadata.annotations.get_or_insert_with(Default::default);
                annotations.insert("edgion.io/hostname-resolution".to_string(), annotation);
            }
        }

        // Resolve listener keys from parentRefs for per-listener route isolation
        if let Some(parent_refs) = route.spec.parent_refs.as_ref().filter(|_| valid_hostnames) {
            let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
            let listener_keys = super::route_utils::resolve_listener_keys(
                parent_refs,
                route_ns,
                route.metadata.namespace.as_deref(),
                _ctx.runtime(),
            );
            if !listener_keys.is_empty() {
                route.spec.resolved_listeners = Some(listener_keys);
            }
        }

        // Per-route listener/hostname resolution result. parentRefs are
        // resolved against the current Gateway listener config; an empty
        // resolved_listeners means no parent Gateway matched at this point
        // (e.g. the route was processed before its parent Gateway).
        tracing::info!(
            component = "http_route_handler",
            route = %format!(
                "{}/{}",
                route.metadata.namespace.as_deref().unwrap_or("default"),
                route.metadata.name.as_deref().unwrap_or("")
            ),
            parent_refs = ?route.spec.parent_refs,
            resolved_listeners = ?route.spec.resolved_listeners,
            resolved_hostnames = ?route.spec.resolved_hostnames,
            "HTTPRoute parentRef listener resolution complete"
        );

        ProcessResult::Continue(route)
    }

    async fn on_change(&self, route: &HTTPRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        let dropped_gateways = update_gateway_route_index(
            ResourceKind::HTTPRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );
        for gateway_key in dropped_gateways {
            ctx.requeue("Gateway", gateway_key).await;
        }

        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        super::requeue_backend_policies_for_owner(&Self::create_resource_ref(route), ctx).await;

        // This route may be someone's delegation child: requeue the parents so
        // they re-flatten with the new child spec.
        let route_key = format!("{route_ns}/{route_name}");
        for parent_key in ctx.runtime().route_delegation_index.parents_of(&route_key, route_ns) {
            ctx.requeue("HTTPRoute", parent_key).await;
        }
    }

    async fn on_delete(&self, route: &HTTPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_value_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        super::requeue_backend_policies_for_owner(&Self::create_resource_ref(route), ctx).await;
        let removed_backend_targets = ctx.runtime().backend_ref_mgr.clear_value_refs(&resource_ref);
        requeue_backend_policies_for_backend_targets(&removed_backend_targets, ctx).await;
        ctx.runtime().service_ref_mgr.clear_value_refs(&resource_ref);

        remove_from_gateway_route_index(ResourceKind::HTTPRoute, route_ns, route_name, ctx.runtime());

        // Delegation cleanup: drop this route's parent-side edges (the delta
        // carries its former children, which must refresh their child-side
        // Accepted status now that this parent is gone), then requeue any
        // parents still delegating to it (they must re-flatten without this
        // child).
        let route_key = format!("{route_ns}/{route_name}");
        let removed = ctx.runtime().route_delegation_index.update_parent(
            &route_key,
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        );
        for child_key in &removed.changed_named {
            if *child_key != route_key {
                ctx.requeue("HTTPRoute", child_key.clone()).await;
            }
        }
        if !removed.changed_namespaces.is_empty() {
            let adapter = RuntimeDelegationLookup {
                runtime: ctx.runtime(),
                grant_owner: None,
            };
            for ns in &removed.changed_namespaces {
                for key in adapter.list_route_keys_in_namespace(ns) {
                    if key != route_key {
                        ctx.requeue("HTTPRoute", key).await;
                    }
                }
            }
        }
        for parent_key in ctx.runtime().route_delegation_index.parents_of(&route_key, route_ns) {
            ctx.requeue("HTTPRoute", parent_key).await;
        }

        remove_from_attached_route_tracker(ResourceKind::HTTPRoute, route_ns, route_name, ctx.runtime());
        // update_status may already have removed the last attachment before a
        // rejected replacement withdraws this cached generation. Requeue its
        // former parents even when the tracker removal above is a no-op.
        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
    }

    fn update_status(&self, route: &mut HTTPRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let source_error = HttpRouteBackendRefs::from_route(route).err();
        if source_error.is_some() {
            _ctx.runtime()
                .plugin_ref_mgr
                .clear_value_refs(&Self::create_resource_ref(route));
            route.spec.resolved_rules = Some(Vec::new());
            route.spec.resolved_listeners = None;
            route.spec.resolved_hostnames = None;
        }
        if source_error.is_none() {
            Self::register_plugin_refs(route, _ctx);
        }
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        // Static per-rule findings drop only the offending rule through
        // `invalid_rule_indices` + PartiallyInvalid. They become
        // Accepted=False only when no effective rule remains.
        let plugin_scope_errors = if source_error.is_none() {
            backend_route_wide_scope_errors(route, _ctx.runtime())
        } else {
            Vec::new()
        };

        // AI admission is Gateway-scoped. A source-only diagnostic neither
        // drops a rule nor activates a fallback, so it must not be represented
        // by Gateway API's PartiallyInvalid condition.

        let mut resolved_refs_errors = validate_http_backend_refs(route, _ctx.runtime());
        if let Ok(refs) = HttpRouteMirrorRefs::from_route(route) {
            resolved_refs_errors.extend(super::request_mirror::errors(refs.effective(), _ctx.runtime()));
        }
        let ref_denied_list = collect_ref_denied_list(route);
        resolved_refs_errors.extend(super::route_utils::collect_ref_denied_errors(
            route_ns,
            ResourceKind::HTTPRoute,
            &ref_denied_list,
        ));

        // Delegation diagnostics (produced by flatten_route during parse):
        // missing named children ride ResolvedRefs; dropped rules ride
        // PartiallyInvalid alongside the per-rule validation findings.
        let mut delegation_dropped: Vec<String> = Vec::new();
        for issue in &route.spec.delegation_issues {
            match issue {
                DelegationIssue::MissingChild { namespace, name } => {
                    resolved_refs_errors.push(ResolvedRefsError::DelegationChildNotFound {
                        kind: "HTTPRoute",
                        namespace: namespace.clone(),
                        name: name.clone(),
                    });
                }
                DelegationIssue::RuleDropped { message } => delegation_dropped.push(message.clone()),
            }
        }

        // Per-rule validation: find rules where ALL backend refs are invalid
        let invalid_details = if source_error.is_none() {
            find_controller_invalid_rule_details(route, route_ns, _ctx.runtime())
        } else {
            Vec::new()
        };
        let invalid_indices: Vec<usize> = invalid_details.iter().map(|(i, _)| *i).collect();
        let total_rules = route.spec.rules.as_ref().map_or(0, |r| r.len());
        let valid_rules = total_rules - invalid_indices.len();

        route.spec.invalid_rule_indices = if invalid_indices.is_empty() {
            None
        } else {
            Some(invalid_indices.clone())
        };

        // Nothing is left to serve for this parent when no rule survives: with
        // delegation the gateway compiles `resolved_rules` verbatim (already
        // pre-filtered during parse), otherwise it compiles `spec.rules` minus
        // `invalid_rule_indices`.
        let serves_no_rule = match &route.spec.resolved_rules {
            Some(resolved) => resolved.is_empty(),
            None => valid_rules == 0,
        };
        // Any static rule failure or delegation drop rejects an empty attachment.
        // AI source diagnostics never withdraw a rule or participate here.
        let attachment_rejects_route =
            (!invalid_details.is_empty() || !delegation_dropped.is_empty()) && serves_no_rule;

        let hostname_error = route.spec.validate_hostnames().err();
        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        if let Some(error) = source_error {
            let message = route_backend_refs::source_error_message(error);
            if !validation_errors.contains(&message) {
                validation_accepted_errors.push(AcceptedError::Invalid { message });
            }
        }
        validation_accepted_errors.extend(plugin_scope_errors.clone());
        if attachment_rejects_route {
            for (index, message) in &invalid_details {
                validation_accepted_errors.push(AcceptedError::UnsupportedValue {
                    message: format!("rules[{index}]: {message}"),
                });
            }
            // PartiallyInvalid is suppressed for a rejected attachment, so keep
            // the dropped child diagnostics in its Accepted condition instead.
            validation_accepted_errors.extend(
                delegation_dropped
                    .iter()
                    .cloned()
                    .map(|message| AcceptedError::UnsupportedValue { message }),
            );
        }
        let route_rejected = source_error.is_some()
            || hostname_error.is_some()
            || attachment_rejects_route
            || !plugin_scope_errors.is_empty();

        let status = route.status.get_or_insert_with(|| HTTPRouteStatus { parents: vec![] });
        let route_hostnames = route.spec.hostnames.as_ref();

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                // Delegation parentRefs (kind: HTTPRoute reverse locks) get
                // their own Accepted semantics below, not Gateway acceptance.
                if !is_gateway_parent_ref(parent_ref) {
                    continue;
                }
                let accepted_result = super::route_utils::validate_parent_ref_accepted(
                    route_ns,
                    parent_ref,
                    route_hostnames,
                    "HTTPRoute",
                    _ctx.runtime(),
                );
                let mut accepted_errors = accepted_result.errors;
                if let Some(error) = hostname_error {
                    accepted_errors.insert(
                        0,
                        AcceptedError::UnsupportedValue {
                            message: error.to_string(),
                        },
                    );
                }
                accepted_errors.extend(validation_accepted_errors.clone());

                // Note: per-rule problems (rewrite/redirect conflict, malformed CORS,
                // unreachable backendRefs) do NOT flip Accepted to False — they are
                // surfaced via PartiallyInvalid (or ResolvedRefs for backend issues).
                // Per Gateway API spec, Accepted reflects whether the Gateway accepts
                // the route at all; flipping it to False would drop the entire route
                // from the data-plane route table.
                //
                // Exceptions are an unsupported native CORS policy that leaves no rule
                // serving and a backend-scoped WAF ownership violation. WAF is
                // request-wide, so a late backend attachment makes the route's
                // election ambiguous even when a sibling rule could serve.

                let ps = super::route_utils::parent_status_for_update(
                    &mut status.parents,
                    parent_ref,
                    route_ns,
                    &self.controller_name,
                );
                let conditions = &mut ps.conditions;
                set_parent_conditions_full(conditions, &accepted_errors, &resolved_refs_errors, generation);

                // Set PartiallyInvalid for per-rule drops while an effective
                // rule remains matchable. Skipped when
                // `attachment_rejects_route` already rejected the whole route: a route
                // that serves nothing must not also claim rules are active.
                let own_invalid = !invalid_indices.is_empty() && valid_rules > 0;
                if !route_rejected && (own_invalid || !delegation_dropped.is_empty()) {
                    let mut reasons: Vec<String> = Vec::new();
                    if own_invalid {
                        reasons.extend(invalid_details.iter().map(|(i, r)| format!("rules[{i}]: {r}")));
                    }
                    reasons.extend(delegation_dropped.iter().cloned());
                    let mut message = reasons.join("; ");
                    if own_invalid {
                        message = format!("{message} ({valid_rules} of {total_rules} rules active)");
                    }
                    message = format!("Dropped Rule(s): {message}");
                    update_condition(
                        conditions,
                        condition_true(
                            condition_types::PARTIALLY_INVALID,
                            condition_reasons::RULE_INVALID,
                            message,
                            generation,
                        ),
                    );
                } else {
                    // PartiallyInvalid is a negative-polarity Gateway API
                    // condition and must be absent unless it is True.
                    conditions.retain(|condition| condition.type_ != condition_types::PARTIALLY_INVALID);
                }
            }

            // Child-side delegation status: each `kind: HTTPRoute` parentRef
            // (reverse lock) reports whether that parent currently delegates
            // to this route.
            let own_key = format!("{route_ns}/{}", route.metadata.name.as_deref().unwrap_or(""));
            let delegating_parents = _ctx.runtime().route_delegation_index.parents_of(&own_key, route_ns);
            for parent_ref in parent_refs.iter().filter(|pr| is_delegation_parent_ref(pr)) {
                let parent_key = format!(
                    "{}/{}",
                    parent_ref.namespace.as_deref().unwrap_or(route_ns),
                    parent_ref.name
                );
                let condition = if source_error.is_some() {
                    condition_false(
                        condition_types::ACCEPTED,
                        condition_reasons::INVALID,
                        "Invalid route reference source",
                        generation,
                    )
                } else if delegating_parents.contains(&parent_key) {
                    condition_true(
                        condition_types::ACCEPTED,
                        "DelegationAccepted",
                        format!("Parent HTTPRoute '{parent_key}' delegates to this route"),
                        generation,
                    )
                } else {
                    condition_false(
                        condition_types::ACCEPTED,
                        "NoMatchingParent",
                        format!("Parent HTTPRoute '{parent_key}' does not currently delegate to this route"),
                        generation,
                    )
                };
                // Match on kind too: a route can carry both a `kind: Gateway`
                // and a `kind: HTTPRoute` parentRef with the same name/ns/
                // section, and the delegation Accepted condition must not
                // overwrite the Gateway-acceptance entry.
                let ps = super::route_utils::parent_status_for_update(
                    &mut status.parents,
                    parent_ref,
                    route_ns,
                    &self.controller_name,
                );
                update_condition(&mut ps.conditions, condition);
            }

            super::route_utils::retain_current_parent_statuses(
                &mut status.parents,
                parent_refs,
                route_ns,
                &self.controller_name,
            );
        } else {
            super::route_utils::retain_current_parent_statuses(
                &mut status.parents,
                &[],
                route_ns,
                &self.controller_name,
            );
        }

        // Update tracker with only accepted parentRefs so attachedRoutes count is accurate.
        super::route_utils::update_tracker_accepted_only(
            ResourceKind::HTTPRoute,
            route_ns,
            route.metadata.name.as_deref().unwrap_or(""),
            route.spec.parent_refs.as_ref().filter(|_| {
                source_error.is_none()
                    && hostname_error.is_none()
                    && route.spec.unsupported_default_gateway_attachment().is_none()
            }),
            route.spec.hostnames.as_ref(),
            _ctx.runtime(),
        );
    }
}

/// Delegation backendRefs (`kind: HTTPRoute`) are excluded — they are not
/// Service backends; their resolution problems are reported through
/// `DelegationIssue` (MissingChild → ResolvedRefs, drops → PartiallyInvalid).
fn validate_http_backend_refs(route: &HTTPRoute, runtime: &ControllerRuntime) -> Vec<ResolvedRefsError> {
    route_backend_refs::validate_targets(
        HttpRouteBackendRefs::from_route(route).map(|references| references.effective()),
        runtime,
    )
}

/// Collect Option<&RefDenied> from all backend refs for shared error collection.
///
/// Scans `effective_rules()` (resolved_rules when present, else spec.rules):
/// on a delegating parent, resolved_rules already contains the non-delegating
/// sibling copies (carrying their ref_denied) PLUS the flattened child
/// backendRefs whose cross-ns Service ref lacks a ReferenceGrant — so this is
/// the complete set with no double-counting of siblings.
fn collect_ref_denied_list(route: &HTTPRoute) -> Vec<Option<&RefDenied>> {
    let mut list = Vec::new();
    for rule in route.effective_rules().into_iter().flatten() {
        if let Some(backend_refs) = &rule.backend_refs {
            for br in backend_refs {
                list.push(br.ref_denied.as_ref());
            }
        }
    }
    list
}

/// Return `Some(reason)` for unsupported backendRef-scoped CORS, or a malformed
/// rule-level `type: CORS` filter: either `cors:` config is absent, or
/// `HTTPCORSFilter::validate()` rejects the configuration. Reason text is
/// operator-readable and lands in the `PartiallyInvalid` condition message.
///
/// Rules flagged here go into `invalid_rule_indices`. With surviving siblings,
/// `PartiallyInvalid=True` leaves those rules active. An unsupported CORS policy
/// with no surviving effective rule escalates to `Accepted=False/UnsupportedValue`.
fn invalid_cors_reason(rule: &edgion_resources::resources::http_route::HTTPRouteRule) -> Option<String> {
    rule.unsupported_cors_reason()
}

/// Apply the same static ForwardAuth validation as Gateway construction before
/// publishing a rule or copying it into a delegation result.
fn invalid_external_auth_reason(rule: &edgion_resources::resources::http_route::HTTPRouteRule) -> Option<String> {
    let filter_reason = |filter: &edgion_resources::resources::http_route::HTTPRouteFilter| {
        if filter.filter_type != HTTPRouteFilterType::ExternalAuth {
            return None;
        }
        filter.external_auth.as_ref()?.validation_error()
    };
    for (index, filter) in rule.filters.iter().flatten().enumerate() {
        if let Some(reason) = filter_reason(filter) {
            return Some(format!("filters[{index}] ExternalAuth: {reason}"));
        }
    }
    for (backend_index, backend) in rule.backend_refs.iter().flatten().enumerate() {
        for (filter_index, filter) in backend.filters.iter().flatten().enumerate() {
            if let Some(reason) = filter_reason(filter) {
                return Some(format!(
                    "backendRefs[{backend_index}].filters[{filter_index}] ExternalAuth: {reason}"
                ));
            }
        }
    }
    None
}

/// Returns a reason string if a rule has an invalid `ExtensionRef` filter.
///
/// `ExtensionRef` has exactly one recognized use: `EdgionPlugins` in the
/// `edgion.io` API group. An empty group denotes the Kubernetes core API group.
/// The former route-level
/// extensionRef-based load-balancing-policy mechanism has been removed; those
/// kinds are now flagged like any other unsupported kind. Load-balancing
/// policy is configured via `EdgionBackendTrafficPolicy`.
///
/// Anything else is a static config error knowable without the runtime store;
/// flagging it here excludes the rule from the data-plane route table instead
/// of leaving it to a silent runtime `GoodNext`.
fn invalid_extension_ref_reason(rule: &edgion_resources::resources::http_route::HTTPRouteRule) -> Option<String> {
    let filter_reason = |f: &edgion_resources::resources::http_route::HTTPRouteFilter| -> Option<String> {
        match validate_extension_ref_filter(
            f.filter_type == HTTPRouteFilterType::ExtensionRef,
            f.extension_ref.as_ref(),
        ) {
            Ok(_) => None,
            Err(ExtensionRefValidationError::MissingConfiguration) => {
                Some("ExtensionRef filter missing 'extensionRef' configuration block".to_string())
            }
            Err(ExtensionRefValidationError::UnexpectedConfiguration) => {
                Some("extensionRef configuration requires filter type ExtensionRef".to_string())
            }
            Err(ExtensionRefValidationError::UnsupportedTarget) => {
                let ext = f.extension_ref.as_ref().expect("unsupported target has a reference");
                if is_supported_edgion_plugins_ref(ext) {
                    return Some("ExtensionRef name must be a valid Kubernetes object name".to_string());
                }
                Some(format!(
                    "ExtensionRef kind '{}' (group '{}') is not supported; only edgion.io/EdgionPlugins is",
                    ext.kind, ext.group
                ))
            }
        }
    };
    if let Some(filters) = rule.filters.as_ref() {
        for f in filters {
            if let Some(reason) = filter_reason(f) {
                return Some(reason);
            }
        }
    }
    if let Some(backend_refs) = rule.backend_refs.as_ref() {
        for (br_idx, br) in backend_refs.iter().enumerate() {
            if let Some(br_filters) = br.filters.as_ref() {
                for f in br_filters {
                    if let Some(reason) = filter_reason(f) {
                        return Some(format!("backendRefs[{br_idx}] {reason}"));
                    }
                }
            }
        }
    }
    None
}

/// Returns a reason string if a rule's `sessionPersistence` block is invalid
/// (bad GEP-2257 `absoluteTimeout`, or Permanent lifetime without
/// `absoluteTimeout`). Kubernetes already rejects these at admission (CRD
/// pattern + CEL); this guards the FileSystem/Etcd modes that have no admission.
/// A flagged rule is dropped via `invalid_rule_indices` rather than silently
/// emitting a never-expiring session token.
fn invalid_session_persistence_reason(rule: &edgion_resources::resources::http_route::HTTPRouteRule) -> Option<String> {
    rule.session_persistence
        .as_ref()
        .and_then(|sp| sp.validate().err())
        .map(|e| format!("sessionPersistence: {e}"))
}

/// Resolve only declarative backend facts, never credentials or runtime objects.
fn resolve_ai_admission(
    reference: EffectiveBackendReference<'_>,
    runtime: &ControllerRuntime,
) -> ResolvedForwardingTarget {
    resolve_ai_admission_with(reference, |target| match target {
        BackendTarget::Service(identity) => {
            if super::route_utils::service_exists(
                identity.scope().namespace().unwrap_or_default(),
                identity.name(),
                runtime,
            ) {
                ResolvedForwardingTarget::Service
            } else {
                ResolvedForwardingTarget::Missing
            }
        }
        BackendTarget::EdgionBackend(identity) => {
            let key = format!(
                "{}/{}",
                identity.scope().namespace().unwrap_or_default(),
                identity.name()
            );
            match runtime
                .processor_registry
                .get("EdgionBackend")
                .and_then(|processor| processor.get_raw_any(&key))
            {
                Some(raw) => match raw.downcast::<EdgionBackend>() {
                    Ok(resource) => ResolvedForwardingTarget::EdgionBackend {
                        spec_valid: resource.spec.validate().is_ok(),
                        provider: resource.spec.ai.provider,
                        has_credential_tpm: resource.spec.ai.has_credential_tpm(),
                        canonical_models: resource.spec.ai.models.iter().map(|model| model.name.clone()).collect(),
                    },
                    Err(_) => ResolvedForwardingTarget::Incompatible,
                },
                None => ResolvedForwardingTarget::Missing,
            }
        }
        _ => ResolvedForwardingTarget::Missing,
    })
}

fn resolve_ai_admission_with(
    reference: EffectiveBackendReference<'_>,
    read: impl FnOnce(BackendTarget<'_>) -> ResolvedForwardingTarget,
) -> ResolvedForwardingTarget {
    if reference.ref_denied().is_some() {
        return ResolvedForwardingTarget::Missing;
    }
    match reference.target() {
        Ok(target @ (BackendTarget::Service(_) | BackendTarget::EdgionBackend(_))) => read(target),
        _ => ResolvedForwardingTarget::Missing,
    }
}

fn populate_ai_admission(route: &mut HTTPRoute, runtime: &ControllerRuntime) {
    let empty_facts = |rules: Option<&Vec<HTTPRouteRule>>| -> Vec<ResolvedAiAdmission> {
        rules
            .into_iter()
            .flatten()
            .map(|rule| ResolvedAiAdmission {
                backends: rule
                    .backend_refs
                    .iter()
                    .flatten()
                    .map(|_| ResolvedForwardingTarget::Missing)
                    .collect(),
            })
            .collect()
    };
    let mut authored = empty_facts(route.spec.rules.as_ref());
    let mut resolved = empty_facts(route.spec.resolved_rules.as_ref());
    if let Ok(references) = HttpRouteBackendRefs::from_route(route) {
        for reference in references.authored().map(|reference| reference.target_view()) {
            let slot = reference.slot();
            authored[slot.rule()].backends[slot.backend()] = resolve_ai_admission(reference, runtime);
        }
        if route.spec.resolved_rules.is_some() {
            for reference in references.effective() {
                let slot = reference.slot();
                resolved[slot.rule()].backends[slot.backend()] = resolve_ai_admission(reference, runtime);
            }
        }
    }
    for (rule, facts) in route.spec.rules.iter_mut().flatten().zip(authored) {
        rule.resolved_ai_admission = Some(facts);
    }
    for (rule, facts) in route.spec.resolved_rules.iter_mut().flatten().zip(resolved) {
        rule.resolved_ai_admission = Some(facts);
    }
}

fn find_controller_invalid_rule_details(
    route: &HTTPRoute,
    route_ns: &str,
    runtime: &ControllerRuntime,
) -> Vec<(usize, String)> {
    let mut details = find_invalid_rule_details(route, route_ns);
    let already_invalid: HashSet<usize> = details.iter().map(|(index, _)| *index).collect();
    for (index, _) in route.spec.rules.iter().flatten().enumerate() {
        if already_invalid.contains(&index) {
            continue;
        }
        if let Some(reason) = backend_plugin_scope_invalid_reason(route, index, false, runtime, false) {
            details.push((index, reason));
            continue;
        }
    }
    details.sort_by_key(|(index, _)| *index);
    details
}

fn backend_plugin_scope_invalid_reason(
    route: &HTTPRoute,
    rule_index: usize,
    effective: bool,
    runtime: &ControllerRuntime,
    route_wide_only: bool,
) -> Option<String> {
    let references = HttpRouteExtensionRefs::from_route(route).ok()?;
    let root = HttpRouteHandler::create_resource_ref(route);
    let fields = || {
        references
            .authored()
            .filter(|_| !effective)
            .chain(references.effective().filter(|_| effective))
            .filter(|field| field.slot().rule() == rule_index)
    };
    // Register the complete valid set before any applicable content inspection.
    for field in fields() {
        if let Some(receipt) = super::route_utils::ExtensionDependencyReceipt::from_field(field) {
            receipt.register(&root, runtime);
        }
    }
    if !effective
        && find_invalid_rule_details(route, route.metadata.namespace.as_deref().unwrap_or("default"))
            .iter()
            .any(|(index, _)| *index == rule_index)
    {
        return None;
    }
    if effective && fields().any(|field| field.validated().is_err()) {
        return Some("invalid ExtensionRef configuration or local target".into());
    }
    fields()
        .filter(|field| field.slot().backend().is_some())
        .find_map(|field| {
            if route_wide_only {
                super::route_utils::backend_request_scope_route_wide_error(runtime, &root, field)
            } else {
                super::route_utils::backend_request_scope_extension_ref_error(runtime, &root, field)
            }
        })
}

fn backend_route_wide_scope_errors(route: &HTTPRoute, runtime: &ControllerRuntime) -> Vec<AcceptedError> {
    let invalid = find_invalid_rule_details(route, route.metadata.namespace.as_deref().unwrap_or("default"))
        .into_iter()
        .map(|(index, _)| index)
        .collect::<HashSet<_>>();
    route
        .spec
        .rules
        .iter()
        .flatten()
        .enumerate()
        .filter(|(index, _)| !invalid.contains(index))
        .filter_map(|(index, _)| {
            backend_plugin_scope_invalid_reason(route, index, false, runtime, true).map(|message| {
                AcceptedError::UnsupportedValue {
                    message: format!("rules[{index}]: {message}"),
                }
            })
        })
        .collect()
}

/// Find rules that are invalid, returning `(index, reason)` for each.
///
/// A rule is invalid if:
/// - Its request transforms violate the shared CRD-derived contract
/// - It has backendRef-scoped CORS, or malformed rule-level CORS (missing `cors:` block or invalid origin)
/// - It has malformed or repeated native header modifiers in a rule or backend filter list
/// - It has invalid standard RequestMirror fields
/// - It has an ExtensionRef filter targeting an unsupported kind (only EdgionPlugins is supported)
///
/// The reason string is rendered into the `PartiallyInvalid` condition message
/// so operators can see *why* a rule was rejected without grepping gateway logs.
///
/// Note: rules whose backend refs are all unreachable (non-Service kind or
/// non-existent) are NOT flagged here — the Gateway still registers them so
/// requests match and get a 500 response (per Gateway API spec).
pub(crate) fn find_invalid_rule_details(route: &HTTPRoute, _route_ns: &str) -> Vec<(usize, String)> {
    let mut details = Vec::new();
    let Some(rules) = &route.spec.rules else {
        return details;
    };
    for (idx, rule) in rules.iter().enumerate() {
        match edgion_resources::authored_request_transform_violations(rule) {
            Ok(violations) => {
                if let Some(violation) = violations.into_iter().next() {
                    details.push((idx, violation.to_string()));
                    continue;
                }
            }
            Err(_) => {
                details.push((idx, "request transform validation contract is unavailable".to_string()));
                continue;
            }
        }
        if let Some(reason) = rule.header_modifier_error() {
            details.push((idx, reason.to_string()));
            continue;
        }
        if let Some(reason) = invalid_cors_reason(rule) {
            details.push((idx, reason));
            continue;
        }
        if let Some(reason) = invalid_external_auth_reason(rule) {
            details.push((idx, reason));
            continue;
        }
        if let Some(reason) = rule.request_mirror_validation_error() {
            details.push((idx, reason));
            continue;
        }
        if let Some(reason) = rule.retry.as_ref().and_then(|retry| retry.validation_error()) {
            details.push((idx, reason.to_string()));
            continue;
        }
        if let Some(reason) = invalid_extension_ref_reason(rule) {
            details.push((idx, reason));
            continue;
        }
        if let Some(reason) = invalid_session_persistence_reason(rule) {
            details.push((idx, reason));
            continue;
        }
    }
    details
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/http_route_test.rs"]
mod tests;

// Locks the dropped-parent-gateway requeue on the coalesced apply path; see
// `tests/coalesced_recreate_test.rs`.
#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/http_route_test/coalesced_recreate_test.rs"]
mod coalesced_recreate_test;

#[cfg(test)]
#[path = "../../../../../tests/support/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs"]
mod test_support;
#[cfg(test)]
use test_support::*;

//! GRPCRoute Handler
//!
//! Handles GRPCRoute resources with ReferenceGrant validation, cross-namespace
//! reference tracking, and GRPCRoute-to-GRPCRoute delegation flattening.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::{is_cross_ns_ref_allowed, validate_grpc_route_if_enabled};
use super::grpc_route_delegation::{flatten_route, DelegationLookup};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_tls_policies_for_route,
    requeue_parent_gateways, update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, set_parent_conditions_full, update_condition,
    AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::GRPCRoute;
use edgion_resources::resources::common::{Condition, ParentReference, RefDenied};
use edgion_resources::resources::grpc_route::{GRPCRouteFilter, GRPCRouteFilterType, GRPCRouteStatus};
use edgion_resources::resources::http_route::{DelegationIssue, RouteParentStatus, GATEWAY_API_GROUP};
use edgion_resources::ResourceKind;

/// True for parentRef entries that attach the route to a Gateway listener
/// (the Gateway API default when group/kind are absent). Delegation
/// parentRefs (`kind: GRPCRoute`, reverse-lock declarations) return false.
fn is_gateway_parent_ref(pr: &ParentReference) -> bool {
    pr.group.as_deref().unwrap_or(GATEWAY_API_GROUP) == GATEWAY_API_GROUP
        && pr.kind.as_deref().unwrap_or("Gateway") == "Gateway"
}

/// True for parentRef entries that reverse-lock delegation to specific parent
/// GRPCRoutes.
fn is_delegation_parent_ref(pr: &ParentReference) -> bool {
    pr.group.as_deref().unwrap_or(GATEWAY_API_GROUP) == GATEWAY_API_GROUP && pr.kind.as_deref() == Some("GRPCRoute")
}

/// `DelegationLookup` backed by the live controller stores.
struct RuntimeDelegationLookup<'a> {
    runtime: &'a ControllerRuntime,
}

impl DelegationLookup for RuntimeDelegationLookup<'_> {
    fn get_route(&self, key: &str) -> Option<Arc<GRPCRoute>> {
        let processor = self.runtime.processor_registry.get("GRPCRoute")?;
        // Visibility must match the wildcard listing (ServerCache): the raw
        // store bypasses the controller's namespace/metadata filters, so a
        // named delegation to a filtered-out child would otherwise expand a
        // route the controller never processes (no on_change → stale forever,
        // and a de-facto namespace-filter bypass).
        if !processor.contains_key(key) {
            return None;
        }
        processor.get_raw_any(key)?.downcast::<GRPCRoute>().ok()
    }

    fn list_route_keys_in_namespace(&self, ns: &str) -> Vec<String> {
        let Some(processor) = self.runtime.processor_registry.get("GRPCRoute") else {
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

    fn cross_ns_ref_allowed(
        &self,
        from_ns: &str,
        group: Option<&str>,
        kind: Option<&str>,
        name: &str,
        to_ns: &str,
    ) -> bool {
        let group = group.map(str::to_string);
        let kind = kind.map(str::to_string);
        is_cross_ns_ref_allowed(
            from_ns,
            "GRPCRoute",
            to_ns,
            group.as_ref(),
            kind.as_ref(),
            name,
            self.runtime,
        )
    }
}

/// GRPCRoute handler
pub struct GrpcRouteHandler {
    controller_name: String,
}

impl GrpcRouteHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }

    fn create_resource_ref(route: &GRPCRoute) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::GRPCRoute,
            route.metadata.namespace.clone(),
            route.metadata.name.clone().unwrap_or_default(),
        )
    }

    /// Register Service backend references for cross-resource requeue.
    ///
    /// `extra_tuples` carries the flattened delegation backendRefs (with
    /// explicit namespaces) so Service changes under delegated children
    /// requeue this parent route directly. Registration replaces the
    /// resource's whole ref set, so own-rule and flattened refs must land in
    /// this single call.
    fn register_service_refs(
        route: &GRPCRoute,
        extra_tuples: &[super::grpc_route_delegation::ServiceBackendTuple],
        ctx: &HandlerContext,
    ) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        let mut backend_refs_list = Vec::new();
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &rule.backend_refs {
                    for br in backend_refs {
                        backend_refs_list.push((
                            br.group.as_deref(),
                            br.kind.as_deref(),
                            br.namespace.as_deref(),
                            br.name.as_str(),
                        ));
                    }
                }
            }
        }
        for (group, kind, namespace, name) in extra_tuples {
            backend_refs_list.push((group.as_deref(), kind.as_deref(), namespace.as_deref(), name.as_str()));
        }

        super::route_utils::register_service_backend_refs(
            ResourceKind::GRPCRoute,
            route_ns,
            route_name,
            &backend_refs_list,
            ctx.runtime(),
        );
    }

    /// Record cross-namespace references from backend_refs.
    ///
    /// `extra_namespaces` carries the cross-namespace targets of flattened
    /// delegation backendRefs so ReferenceGrant changes in those namespaces
    /// requeue this parent route. Registration replaces the resource's whole
    /// ref set, so own-rule and flattened targets must land in this single call.
    fn record_cross_ns_refs(route: &GRPCRoute, extra_namespaces: &HashSet<String>, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().cross_ns_ref_mgr;

        // Clear old references first (handles updates)
        manager.clear_resource_refs(&resource_ref);

        // Collect cross-namespace references from rules
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &rule.backend_refs {
                    for backend_ref in backend_refs {
                        if let Some(backend_ns) = &backend_ref.namespace {
                            if backend_ns != route_ns {
                                manager.add_ref(backend_ns.clone(), resource_ref.clone());
                            }
                        }
                    }
                }
            }
        }
        for ns in extra_namespaces {
            manager.add_ref(ns.clone(), resource_ref.clone());
        }
    }
}

impl Default for GrpcRouteHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<GRPCRoute> for GrpcRouteHandler {
    fn validate(&self, route: &GRPCRoute, _ctx: &HandlerContext) -> Vec<String> {
        let errors = validate_grpc_route_if_enabled(route);
        // Note: backend ref validation (service existence, kind check) is handled by
        // update_status() via ResolvedRefs condition and per-rule invalid_rule_indices.
        errors
    }

    fn preparse(&self, route: &mut GRPCRoute, _ctx: &HandlerContext) -> Vec<String> {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        update_gateway_route_index(
            ResourceKind::GRPCRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
            _ctx.runtime(),
        );
        Vec::new()
    }

    async fn parse(&self, mut route: GRPCRoute, _ctx: &HandlerContext) -> ProcessResult<GRPCRoute> {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let route_ns = route_ns.as_str();

        // Mark denied cross-namespace references on the operator-authored rules.
        // MUST run BEFORE the delegation flatten: flatten clones non-delegating
        // sibling rules verbatim into resolved_rules, and the gateway compiles
        // resolved_rules INSTEAD of spec.rules — marking afterwards would leave
        // the compiled copies with ref_denied=None (ReferenceGrant bypass).
        if let Some(rules) = &mut route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &mut rule.backend_refs {
                    for backend_ref in backend_refs {
                        // Delegation refs (kind: GRPCRoute) are not Service
                        // backends: cross-namespace delegation is authorized
                        // by the parent itself (no ReferenceGrant), so they
                        // must never be marked ref_denied.
                        if backend_ref.is_route_delegation_ref() {
                            backend_ref.ref_denied = None;
                            continue;
                        }
                        if let Some(backend_ns) = &backend_ref.namespace {
                            if backend_ns != route_ns {
                                let allowed = is_cross_ns_ref_allowed(
                                    route_ns,
                                    "GRPCRoute",
                                    backend_ns,
                                    backend_ref.group.as_ref(),
                                    backend_ref.kind.as_ref(),
                                    &backend_ref.name,
                                    _ctx.runtime(),
                                );
                                if !allowed {
                                    backend_ref.ref_denied = Some(RefDenied {
                                        target_namespace: backend_ns.clone(),
                                        target_name: backend_ref.name.clone(),
                                        reason: Some("NoMatchingReferenceGrant".to_string()),
                                    });
                                } else {
                                    // Clear ref_denied if now allowed (e.g., ReferenceGrant was added)
                                    backend_ref.ref_denied = None;
                                }
                            } else {
                                // Same namespace - always allowed
                                backend_ref.ref_denied = None;
                            }
                        } else {
                            // No namespace specified - same namespace assumed
                            backend_ref.ref_denied = None;
                        }
                    }
                }
            }
        }

        // GRPCRoute delegation: expand delegation trees into resolved_rules.
        // Runs early so the flattened Service/cross-ns refs land in the single
        // registration pass below. resolved_rules/delegation_issues are
        // controller-derived: clear any user-provided or stale values.
        route.spec.resolved_rules = None;
        route.spec.delegation_issues = Vec::new();
        let delegation_outcome = {
            let adapter = RuntimeDelegationLookup {
                runtime: _ctx.runtime(),
            };
            flatten_route(&route, &adapter)
        };

        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route, &delegation_outcome.cross_ns_target_namespaces, _ctx);

        // Register Service backend references for cross-resource requeue
        Self::register_service_refs(&route, &delegation_outcome.service_backend_tuples, _ctx);

        // Maintain the delegation index (also clears stale edges when the
        // route no longer delegates) and requeue children whose delegation
        // edges changed so their child-side status stays fresh.
        {
            let route_key = format!("{route_ns}/{}", route.metadata.name.as_deref().unwrap_or(""));
            let delta = _ctx.runtime().grpc_route_delegation_index.update_parent(
                &route_key,
                delegation_outcome.named_children.clone(),
                delegation_outcome.wildcard_namespaces.clone(),
            );
            for child_key in &delta.changed_named {
                if *child_key != route_key {
                    _ctx.requeue("GRPCRoute", child_key.clone()).await;
                }
            }
            if !delta.changed_namespaces.is_empty() {
                let adapter = RuntimeDelegationLookup {
                    runtime: _ctx.runtime(),
                };
                for ns in &delta.changed_namespaces {
                    for key in adapter.list_route_keys_in_namespace(ns) {
                        if key != route_key {
                            _ctx.requeue("GRPCRoute", key).await;
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
                };
                let vanished = delegation_outcome
                    .expanded_children
                    .iter()
                    .any(|key| adapter.get_route(key).is_none());
                if vanished {
                    _ctx.requeue("GRPCRoute", route_key.clone()).await;
                }
            }
        }
        route.spec.resolved_rules = delegation_outcome.resolved_rules;
        route.spec.delegation_issues = delegation_outcome.issues;

        // Treat resolved_listeners as controller-derived state.
        // Clear any user-provided or stale value before recomputing from current parentRefs.
        route.spec.resolved_listeners = None;

        // Resolve effective hostnames (intersection with Gateway listener hostnames)
        if let Some(parent_refs) = &route.spec.parent_refs {
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
        if let Some(parent_refs) = &route.spec.parent_refs {
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

        ProcessResult::Continue(route)
    }

    async fn on_change(&self, route: &GRPCRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        update_gateway_route_index(
            ResourceKind::GRPCRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );

        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        requeue_backend_tls_policies_for_route(ResourceKind::GRPCRoute, route_ns, route_name, ctx).await;

        // This route may be someone's delegation child: requeue the parents so
        // they re-flatten with the new child spec.
        let route_key = format!("{route_ns}/{route_name}");
        for parent_key in ctx
            .runtime()
            .grpc_route_delegation_index
            .parents_of(&route_key, route_ns)
        {
            ctx.requeue("GRPCRoute", parent_key).await;
        }
    }

    async fn on_delete(&self, route: &GRPCRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        requeue_backend_tls_policies_for_route(ResourceKind::GRPCRoute, route_ns, route_name, ctx).await;
        super::route_utils::clear_service_backend_refs(ResourceKind::GRPCRoute, route_ns, route_name, ctx.runtime());

        remove_from_gateway_route_index(ResourceKind::GRPCRoute, route_ns, route_name, ctx.runtime());

        // Delegation cleanup: drop this route's parent-side edges (the delta
        // carries its former children, which must refresh their child-side
        // Accepted status now that this parent is gone), then requeue any
        // parents still delegating to it (they must re-flatten without this
        // child).
        let route_key = format!("{route_ns}/{route_name}");
        let removed = ctx.runtime().grpc_route_delegation_index.update_parent(
            &route_key,
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        );
        for child_key in &removed.changed_named {
            if *child_key != route_key {
                ctx.requeue("GRPCRoute", child_key.clone()).await;
            }
        }
        if !removed.changed_namespaces.is_empty() {
            let adapter = RuntimeDelegationLookup { runtime: ctx.runtime() };
            for ns in &removed.changed_namespaces {
                for key in adapter.list_route_keys_in_namespace(ns) {
                    if key != route_key {
                        ctx.requeue("GRPCRoute", key).await;
                    }
                }
            }
        }
        for parent_key in ctx
            .runtime()
            .grpc_route_delegation_index
            .parents_of(&route_key, route_ns)
        {
            ctx.requeue("GRPCRoute", parent_key).await;
        }

        let tracker_changed =
            remove_from_attached_route_tracker(ResourceKind::GRPCRoute, route_ns, route_name, ctx.runtime());
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_status(&self, route: &mut GRPCRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

        let backend_ref_tuples = collect_backend_ref_tuples(route);
        let mut resolved_refs_errors =
            super::route_utils::validate_backend_refs(route_ns, &backend_ref_tuples, _ctx.runtime());
        let ref_denied_list = collect_ref_denied_list(route);
        resolved_refs_errors.extend(super::route_utils::collect_ref_denied_errors(
            route_ns,
            ResourceKind::GRPCRoute,
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
                        kind: "GRPCRoute",
                        namespace: namespace.clone(),
                        name: name.clone(),
                    });
                }
                DelegationIssue::RuleDropped { message } => delegation_dropped.push(message.clone()),
            }
        }

        // Per-rule validation: find rules where ALL backend refs are invalid
        let invalid_indices = find_invalid_rule_indices(route, route_ns);
        let total_rules = route.spec.rules.as_ref().map_or(0, |r| r.len());
        let valid_rules = total_rules - invalid_indices.len();

        route.spec.invalid_rule_indices = if invalid_indices.is_empty() {
            None
        } else {
            Some(invalid_indices.clone())
        };

        let status = route.status.get_or_insert_with(|| GRPCRouteStatus { parents: vec![] });
        let route_hostnames = route.spec.hostnames.as_ref();

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                // Delegation parentRefs (kind: GRPCRoute reverse locks) get
                // their own Accepted semantics below, not Gateway acceptance.
                if !is_gateway_parent_ref(parent_ref) {
                    continue;
                }
                let accepted_result = super::route_utils::validate_parent_ref_accepted(
                    route_ns,
                    parent_ref,
                    route_hostnames,
                    "GRPCRoute",
                    _ctx.runtime(),
                );
                let mut accepted_errors = accepted_result.errors;
                accepted_errors.extend(validation_accepted_errors.clone());

                // Note: all_rules_invalid due to backend ref issues should NOT
                // affect Accepted status. Per Gateway API spec, Accepted reflects
                // whether the Gateway accepts the route. Backend issues only affect
                // ResolvedRefs.

                let conditions = if let Some(ps) = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name
                        && ps.parent_ref.namespace == parent_ref.namespace
                        && ps.parent_ref.section_name == parent_ref.section_name
                }) {
                    set_parent_conditions_full(&mut ps.conditions, &accepted_errors, &resolved_refs_errors, generation);
                    &mut ps.conditions
                } else {
                    let mut conditions = Vec::new();
                    set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_refs_errors, generation);

                    status.parents.push(RouteParentStatus {
                        parent_ref: parent_ref.clone(),
                        controller_name: self.controller_name.clone(),
                        conditions,
                    });
                    &mut status
                        .parents
                        .last_mut()
                        .expect("just pushed; Vec is non-empty")
                        .conditions
                };

                // Set PartiallyInvalid condition (own-rule findings plus
                // delegation drops — a dropped delegated rule never flips
                // Accepted, matching the per-rule precedent above).
                let own_invalid = !invalid_indices.is_empty() && valid_rules > 0;
                if own_invalid || !delegation_dropped.is_empty() {
                    let mut reasons: Vec<String> = Vec::new();
                    if own_invalid {
                        reasons.push(format!(
                            "Rules at indices {:?} are invalid; {} of {} rules are active",
                            invalid_indices, valid_rules, total_rules
                        ));
                    }
                    reasons.extend(delegation_dropped.iter().cloned());
                    let message = reasons.join("; ");
                    update_condition(
                        conditions,
                        condition_true(
                            condition_types::PARTIALLY_INVALID,
                            condition_reasons::RULE_INVALID,
                            message,
                            generation,
                        ),
                    );
                } else if conditions.iter().any(|c| c.type_ == condition_types::PARTIALLY_INVALID) {
                    update_condition(
                        conditions,
                        condition_false(
                            condition_types::PARTIALLY_INVALID,
                            condition_reasons::ALL_RULES_VALID,
                            "All rules are valid",
                            generation,
                        ),
                    );
                }
            }

            // Child-side delegation status: each `kind: GRPCRoute` parentRef
            // (reverse lock) reports whether that parent currently delegates
            // to this route.
            let own_key = format!("{route_ns}/{}", route.metadata.name.as_deref().unwrap_or(""));
            let delegating_parents = _ctx
                .runtime()
                .grpc_route_delegation_index
                .parents_of(&own_key, route_ns);
            for parent_ref in parent_refs.iter().filter(|pr| is_delegation_parent_ref(pr)) {
                let parent_key = format!(
                    "{}/{}",
                    parent_ref.namespace.as_deref().unwrap_or(route_ns),
                    parent_ref.name
                );
                let condition = delegation_accepted_condition(&parent_key, &delegating_parents, generation);
                // Match on kind too: a route can carry both a `kind: Gateway`
                // and a `kind: GRPCRoute` parentRef with the same name/ns/
                // section, and the delegation Accepted condition must not
                // overwrite the Gateway-acceptance entry.
                if let Some(ps) = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name
                        && ps.parent_ref.namespace == parent_ref.namespace
                        && ps.parent_ref.section_name == parent_ref.section_name
                        && ps.parent_ref.kind == parent_ref.kind
                }) {
                    update_condition(&mut ps.conditions, condition);
                } else {
                    let mut conditions = Vec::new();
                    update_condition(&mut conditions, condition);
                    status.parents.push(RouteParentStatus {
                        parent_ref: parent_ref.clone(),
                        controller_name: self.controller_name.clone(),
                        conditions,
                    });
                }
            }

            super::route_utils::retain_current_parent_statuses(&mut status.parents, parent_refs);
        } else {
            status.parents.clear();
        }

        // Update tracker with only accepted parentRefs so attachedRoutes count is accurate.
        super::route_utils::update_tracker_accepted_only(
            ResourceKind::GRPCRoute,
            route_ns,
            route.metadata.name.as_deref().unwrap_or(""),
            route.spec.parent_refs.as_ref(),
            route.spec.hostnames.as_ref(),
            _ctx.runtime(),
        );
    }
}

/// Extract (group, kind, namespace, name) tuples from all backend refs for shared validation.
///
/// Delegation backendRefs (`kind: GRPCRoute`) are excluded — they are not
/// Service backends; their resolution problems are reported through
/// `DelegationIssue` (MissingChild → ResolvedRefs, drops → PartiallyInvalid).
fn collect_backend_ref_tuples(route: &GRPCRoute) -> Vec<(Option<&str>, Option<&str>, Option<&str>, &str)> {
    let mut tuples = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for br in backend_refs {
                    if br.is_route_delegation_ref() {
                        continue;
                    }
                    tuples.push((
                        br.group.as_deref(),
                        br.kind.as_deref(),
                        br.namespace.as_deref(),
                        br.name.as_str(),
                    ));
                }
            }
        }
    }
    tuples
}

/// Collect Option<&RefDenied> from all backend refs for shared error collection.
///
/// Scans `effective_rules()` (resolved_rules when present, else spec.rules):
/// on a delegating parent, resolved_rules already contains the non-delegating
/// sibling copies (carrying their ref_denied) PLUS the flattened child
/// backendRefs whose cross-ns Service ref lacks a ReferenceGrant — so this is
/// the complete set with no double-counting of siblings.
fn collect_ref_denied_list(route: &GRPCRoute) -> Vec<Option<&RefDenied>> {
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

/// Compute the Accepted condition for a `kind: GRPCRoute` reverse-lock
/// parentRef, given the set of parents currently delegating to this route (as
/// returned by `grpc_route_delegation_index.parents_of`). Pure/testable
/// without a ControllerRuntime.
fn delegation_accepted_condition(
    parent_key: &str,
    delegating_parents: &[String],
    generation: Option<i64>,
) -> Condition {
    if delegating_parents.iter().any(|p| p == parent_key) {
        condition_true(
            condition_types::ACCEPTED,
            "DelegationAccepted",
            format!("Parent GRPCRoute '{parent_key}' delegates to this route"),
            generation,
        )
    } else {
        condition_false(
            condition_types::ACCEPTED,
            "NoMatchingParent",
            format!("Parent GRPCRoute '{parent_key}' does not currently delegate to this route"),
            generation,
        )
    }
}

/// Check if a filter list has the same single-instance filter type more than once.
///
/// Per Gateway API spec for GRPCRoute, "specifying the same filter multiple times
/// is not supported unless explicitly indicated in the filter."
/// `ExtensionRef` is explicitly allowed to repeat;
/// `RequestHeaderModifier` and `ResponseHeaderModifier` are not.
fn has_duplicate_filter_conflict(filters: Option<&Vec<GRPCRouteFilter>>) -> bool {
    let Some(filters) = filters else { return false };
    let mut seen_request_header = false;
    let mut seen_response_header = false;
    for f in filters {
        match f.filter_type {
            GRPCRouteFilterType::RequestHeaderModifier => {
                if seen_request_header {
                    return true;
                }
                seen_request_header = true;
            }
            GRPCRouteFilterType::ResponseHeaderModifier => {
                if seen_response_header {
                    return true;
                }
                seen_response_header = true;
            }
            GRPCRouteFilterType::ExtensionRef => {}
        }
    }
    false
}

/// True if any `ExtensionRef` filter in the list points at a kind that is not
/// recognized.
///
/// `ExtensionRef` has exactly one recognized use: `EdgionPlugins` (group empty
/// or `edgion.io`) — a plugin reference. The former route-level
/// extensionRef-based load-balancing-policy mechanism has been removed; those
/// kinds are now flagged like any other unsupported kind. Load-balancing
/// policy is configured via `EdgionBackendTrafficPolicy`.
///
/// Anything else is a static config error; the gRPC side has no reason-string
/// plumbing, so this only contributes the rule index to `invalid_rule_indices`.
fn has_unsupported_extension_ref(filters: Option<&Vec<GRPCRouteFilter>>) -> bool {
    let Some(filters) = filters else { return false };
    for f in filters {
        if f.filter_type != GRPCRouteFilterType::ExtensionRef {
            continue;
        }
        if let Some(ext) = f.extension_ref.as_ref() {
            let group_ok = ext.group.is_empty() || ext.group == "edgion.io";
            let recognized = group_ok && ext.kind == "EdgionPlugins";
            if !recognized {
                return true;
            }
        }
    }
    false
}

/// Find rule indices where the rule is invalid.
///
/// A rule is invalid if it (or any of its backend refs) has a duplicate single-instance
/// filter type within a single filter list. Rules with invalid backends (non-Service kind
/// or non-existent) are NOT marked as invalid here — the Gateway still registers them so
/// requests match and get an error response (per Gateway API spec).
///
/// Scope: dup-same-type detection is conservatively limited to within a single filter list
/// (rule.filters or one backend_ref.filters). Cross-level same-type (e.g. one
/// RequestHeaderModifier on the rule + one on a backend) is not flagged because the
/// Gateway API spec does not define it as a conflict for GRPCRoute. Note: GRPCRoute also
/// has no mutually-exclusive filter pair like HTTPRoute's URLRewrite + RequestRedirect.
pub(crate) fn find_invalid_rule_indices(route: &GRPCRoute, _route_ns: &str) -> Vec<usize> {
    let mut invalid_indices = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for (idx, rule) in rules.iter().enumerate() {
            if has_duplicate_filter_conflict(rule.filters.as_ref()) {
                invalid_indices.push(idx);
                continue;
            }
            if has_unsupported_extension_ref(rule.filters.as_ref()) {
                invalid_indices.push(idx);
                continue;
            }
            if let Some(backend_refs) = &rule.backend_refs {
                let backend_conflict = backend_refs.iter().any(|br| {
                    has_duplicate_filter_conflict(br.filters.as_ref())
                        || has_unsupported_extension_ref(br.filters.as_ref())
                });
                if backend_conflict {
                    invalid_indices.push(idx);
                }
            }
        }
    }
    invalid_indices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use serde_json::json;
    use std::sync::Arc;

    fn make_ctx() -> HandlerContext {
        HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            3,
            20,
            Arc::new(ControllerRuntime::default()),
        )
    }

    #[tokio::test]
    async fn parse_clears_stale_resolved_listeners_when_current_parent_refs_cannot_resolve() {
        let handler = GrpcRouteHandler::default();
        let ctx = make_ctx();
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {
                "name": "test-route",
                "namespace": "default"
            },
            "spec": {
                "parentRefs": [
                    {
                        "name": "missing-gateway"
                    }
                ]
            }
        }))
        .expect("route should deserialize");

        let parsed = match handler.parse(route, &ctx).await {
            ProcessResult::Continue(route) => route,
            other => panic!("unexpected parse result: {other:?}"),
        };

        assert_eq!(
            parsed.spec.resolved_listeners, None,
            "stale or user-provided resolvedListeners must be cleared when current parentRefs cannot be resolved"
        );
    }

    fn route_with_rules(rules: serde_json::Value) -> GRPCRoute {
        serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "t", "namespace": "default" },
            "spec": { "rules": rules }
        }))
        .expect("route should deserialize")
    }

    #[test]
    fn duplicate_request_header_modifier_marks_rule_invalid() {
        let route = route_with_rules(json!([
            {
                "filters": [
                    { "type": "RequestHeaderModifier", "requestHeaderModifier": { "set": [] } },
                    { "type": "RequestHeaderModifier", "requestHeaderModifier": { "remove": ["x"] } }
                ]
            }
        ]));
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
    }

    #[test]
    fn duplicate_response_header_modifier_marks_rule_invalid() {
        let route = route_with_rules(json!([
            {
                "filters": [
                    { "type": "ResponseHeaderModifier", "responseHeaderModifier": { "set": [] } },
                    { "type": "ResponseHeaderModifier", "responseHeaderModifier": { "remove": ["x"] } }
                ]
            }
        ]));
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
    }

    #[test]
    fn duplicate_filter_in_backend_ref_marks_rule_invalid() {
        let route = route_with_rules(json!([
            {
                "backendRefs": [
                    {
                        "name": "svc-a",
                        "filters": [
                            { "type": "RequestHeaderModifier", "requestHeaderModifier": { "set": [] } },
                            { "type": "RequestHeaderModifier", "requestHeaderModifier": { "remove": ["x"] } }
                        ]
                    }
                ]
            }
        ]));
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
    }

    #[test]
    fn repeated_extension_ref_is_allowed() {
        // ExtensionRef is explicitly allowed to appear multiple times.
        // ExtensionRef filters must point at a supported kind (EdgionPlugins) to be valid.
        let route = route_with_rules(json!([
            {
                "filters": [
                    { "type": "ExtensionRef", "extensionRef": { "group": "edgion.io", "kind": "EdgionPlugins", "name": "a" } },
                    { "type": "ExtensionRef", "extensionRef": { "kind": "EdgionPlugins", "name": "b" } }
                ]
            }
        ]));
        assert!(super::find_invalid_rule_indices(&route, "default").is_empty());
    }

    #[test]
    fn extension_ref_unsupported_kind_marks_rule_invalid() {
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "r", "namespace": "default" },
            "spec": {
                "rules": [{
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": { "group": "edgion.io", "kind": "NotAPlugin", "name": "x" }
                    }]
                }]
            }
        }))
        .expect("route should deserialize");
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
    }

    #[test]
    fn extension_ref_unsupported_kind_on_backend_ref_marks_rule_invalid() {
        // The backendRef-level ExtensionRef check must also flag the rule invalid.
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "r", "namespace": "default" },
            "spec": {
                "rules": [{
                    "backendRefs": [{
                        "name": "svc-a",
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": { "group": "edgion.io", "kind": "NotAPlugin", "name": "x" }
                        }]
                    }]
                }]
            }
        }))
        .expect("route should deserialize");
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
    }

    #[test]
    fn extension_ref_lb_policy_on_backend_ref_now_flagged_as_unsupported() {
        // Regression: the route-level extensionRef-based load-balancing-policy
        // mechanism was removed, so a formerly-exempt kind now fails the same
        // "only EdgionPlugins is supported" gate as any other unknown kind.
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "r", "namespace": "default" },
            "spec": {
                "rules": [{
                    "backendRefs": [{
                        "name": "svc-a",
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": { "group": "edgion.io", "kind": "LBPolicyConsistentHash", "name": "header.x-user-id" }
                        }]
                    }]
                }]
            }
        }))
        .expect("route should deserialize");
        assert_eq!(
            super::find_invalid_rule_indices(&route, "default"),
            vec![0],
            "LBPolicy backendRef ExtensionRef must now be flagged invalid"
        );
    }

    #[test]
    fn cross_level_dup_documents_intentional_conservative_scope() {
        // GRPC spec does not define cross-level (rule + backend) duplicate as conflict.
        // Same single-instance filter type appearing once in rule and once in backend is allowed.
        // This test pins the intentional scope choice — do not relax it without revisiting spec.
        let route = route_with_rules(json!([
            {
                "filters": [
                    { "type": "RequestHeaderModifier", "requestHeaderModifier": { "set": [] } }
                ],
                "backendRefs": [
                    {
                        "name": "svc-a",
                        "filters": [
                            { "type": "RequestHeaderModifier", "requestHeaderModifier": { "remove": ["x"] } }
                        ]
                    }
                ]
            }
        ]));
        assert!(super::find_invalid_rule_indices(&route, "default").is_empty());
    }

    // =====================================================================
    // Delegation wiring tests (mirrors http_route.rs's delegation coverage)
    // =====================================================================

    #[test]
    fn collect_backend_ref_tuples_excludes_delegation_refs() {
        // Regression: a delegating backendRef (kind: GRPCRoute) must never
        // reach validate_backend_refs — it is not a Service backend and would
        // otherwise be flagged ResolvedRefs=False/InvalidKind on a valid
        // delegating parent.
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "r", "namespace": "default" },
            "spec": {
                "rules": [{
                    "matches": [{"method": {"service": "svc.Billing"}}],
                    "backendRefs": [{
                        "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                        "name": "child"
                    }]
                }]
            }
        }))
        .expect("route should deserialize");
        assert!(super::collect_backend_ref_tuples(&route).is_empty());
    }

    #[test]
    fn collect_ref_denied_list_uses_effective_rules() {
        // Regression: once resolved_rules is present, the parent's status
        // must scan the flattened copies (which carry the real ref_denied
        // state the gateway compiles), not the pre-flatten spec.rules.
        let mut route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "r", "namespace": "default" },
            "spec": { "rules": [] }
        }))
        .expect("route should deserialize");
        route.spec.resolved_rules = Some(
            serde_json::from_value(json!([{
                "matches": [{"method": {"service": "svc.Billing"}}],
                "backendRefs": [{
                    "name": "svc",
                    "namespace": "other-ns",
                    "port": 80,
                    "refDenied": {
                        "targetNamespace": "other-ns",
                        "targetName": "svc",
                        "reason": "NoMatchingReferenceGrant"
                    }
                }]
            }]))
            .expect("resolved_rules fixture should deserialize"),
        );
        let list = super::collect_ref_denied_list(&route);
        assert_eq!(list.len(), 1);
        assert!(
            list[0].is_some(),
            "ref_denied on resolved_rules must be picked up via effective_rules()"
        );
    }

    #[test]
    fn delegation_accepted_condition_true_when_parent_delegates() {
        let delegating = vec!["infra/parent".to_string()];
        let cond = super::delegation_accepted_condition("infra/parent", &delegating, Some(3));
        assert_eq!(cond.status, "True");
        assert_eq!(cond.reason, "DelegationAccepted");
        assert!(cond.message.contains("infra/parent"));
    }

    #[test]
    fn delegation_accepted_condition_false_when_parent_does_not_delegate() {
        let delegating: Vec<String> = vec![];
        let cond = super::delegation_accepted_condition("infra/parent", &delegating, Some(3));
        assert_eq!(cond.status, "False");
        assert_eq!(cond.reason, "NoMatchingParent");
    }

    #[tokio::test]
    async fn parse_marks_ref_denied_before_flatten_copies_siblings() {
        // Regression (P0): flatten copies non-delegating sibling rules into
        // resolved_rules, which the gateway compiles INSTEAD of spec.rules.
        // The ReferenceGrant marking must therefore run BEFORE flatten so the
        // copies carry ref_denied — otherwise an ungranted cross-ns Service
        // ref on a delegating parent would be silently routable.
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {"name": "parent", "namespace": "infra"},
            "spec": {
                "rules": [
                    {
                        "matches": [{"method": {"service": "svc.Plain"}}],
                        "backendRefs": [{"name": "secret-svc", "namespace": "secret-ns", "port": 80}]
                    },
                    {
                        "matches": [{"method": {"service": "svc.Billing"}}],
                        "backendRefs": [{
                            "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                            "name": "*", "namespace": "billing"
                        }]
                    }
                ]
            }
        }))
        .unwrap();

        let handler = GrpcRouteHandler::default();
        let ctx = make_ctx();
        let ProcessResult::Continue(parsed) = handler.parse(route, &ctx).await else {
            panic!("parse must continue");
        };

        // No ReferenceGrant exists in the default runtime → the sibling's
        // cross-ns Service ref is denied on spec.rules...
        let spec_br = &parsed.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0];
        assert!(spec_br.ref_denied.is_some(), "spec.rules sibling must be ref_denied");

        // ...AND on the resolved_rules copy the gateway actually compiles.
        let resolved = parsed
            .spec
            .resolved_rules
            .as_ref()
            .expect("delegating parent has resolved_rules");
        let resolved_sibling = resolved
            .iter()
            .find(|r| {
                r.matches
                    .iter()
                    .flatten()
                    .any(|m| m.method.as_ref().and_then(|mm| mm.service.as_deref()) == Some("svc.Plain"))
            })
            .expect("sibling rule copied into resolved_rules");
        let resolved_br = &resolved_sibling.backend_refs.as_ref().unwrap()[0];
        assert!(
            resolved_br.ref_denied.is_some(),
            "resolved_rules sibling copy must carry ref_denied (gateway compiles this copy)"
        );
    }

    #[tokio::test]
    async fn parse_never_marks_delegation_refs_ref_denied() {
        // Regression: a cross-namespace delegation backendRef (kind: GRPCRoute)
        // must not go through the ReferenceGrant check — delegation is
        // authorized by the parent opt-in, and a spurious ref_denied surfaced
        // as ResolvedRefs=False/RefNotPermitted on the parent status.
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {"name": "parent", "namespace": "infra"},
            "spec": {
                "rules": [{
                    "matches": [{"method": {"service": "svc.Billing"}}],
                    "backendRefs": [{
                        "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                        "name": "*", "namespace": "billing"
                    }]
                }]
            }
        }))
        .unwrap();

        let handler = GrpcRouteHandler::default();
        let ctx = make_ctx();
        let result = handler.parse(route, &ctx).await;
        let ProcessResult::Continue(parsed) = result else {
            panic!("parse must continue");
        };
        let br = &parsed.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0];
        assert!(
            br.ref_denied.is_none(),
            "delegation ref must not be ref_denied: {:?}",
            br.ref_denied
        );
        // And the tuple collection excludes it (no InvalidKind on status path).
        assert!(collect_backend_ref_tuples(&parsed).is_empty());
    }

    #[tokio::test]
    async fn parse_updates_grpc_route_delegation_index_and_on_delete_removes_it() {
        // Handler-level index bookkeeping: parse() must register the
        // delegation edge in grpc_route_delegation_index (separate instance
        // from route_delegation_index, so it doesn't collide with HTTPRoute
        // keys), and on_delete() must clear it via update_parent(∅,∅).
        let handler = GrpcRouteHandler::default();
        let ctx = make_ctx();
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {"name": "parent", "namespace": "infra"},
            "spec": {
                "rules": [{
                    "matches": [{"method": {"service": "svc.Billing"}}],
                    "backendRefs": [{
                        "group": "gateway.networking.k8s.io", "kind": "GRPCRoute",
                        "name": "child", "namespace": "billing"
                    }]
                }]
            }
        }))
        .unwrap();

        let parsed = match handler.parse(route, &ctx).await {
            ProcessResult::Continue(route) => route,
            other => panic!("unexpected parse result: {other:?}"),
        };

        assert_eq!(
            ctx.runtime()
                .grpc_route_delegation_index
                .parents_of("billing/child", "billing"),
            vec!["infra/parent".to_string()],
            "parse must register the delegation edge in grpc_route_delegation_index"
        );

        handler.on_delete(&parsed, &ctx).await;

        assert!(
            ctx.runtime()
                .grpc_route_delegation_index
                .parents_of("billing/child", "billing")
                .is_empty(),
            "on_delete must remove the parent's delegation edges"
        );
    }
}

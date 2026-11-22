//! GRPCRoute Handler
//!
//! Handles GRPCRoute resources with ReferenceGrant validation, cross-namespace
//! reference tracking, and GRPCRoute-to-GRPCRoute delegation validation and
//! key publication (no flattening; see `grpc_route_delegation.rs`).

use std::collections::HashSet;

use super::super::ref_grant::validate_grpc_route_if_enabled;
use super::grpc_route_delegation::{grpc_child_rules, grpc_delegation_facts};
use super::route_backend_refs::{self, BackendDependencyReceipt};
use super::route_delegation::{
    child_entry_verdict, edge_message, parent_named_target_errors, resolve_child_keys, route_exists,
    sync_delegation_graph, ChildEntryVerdict, GrpcDelegationFacts, DELEGATION_ACCEPTED, INVALID_PARENT_REFS,
};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_policies_for_owner,
    requeue_backend_policies_for_services, requeue_parent_gateways, update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_reasons, condition_true, condition_types, set_parent_conditions_full, update_condition, AcceptedError,
    HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::GRPCRoute;
use edgion_resources::resources::common::{ParentReference, RefDenied};
use edgion_resources::resources::grpc_route::{GRPCRouteFilter, GRPCRouteFilterType, GRPCRouteStatus};
use edgion_resources::resources::http_route::GATEWAY_API_GROUP;
use edgion_resources::resources::refs::extension_ref::validate_extension_ref_filter;
use edgion_resources::resources::refs::route_fields::{GrpcRouteBackendRefs, GrpcRouteExtensionRefs};
use edgion_resources::resources::route_delegation::{ParentRefShape, RouteKindName};
use edgion_resources::ResourceKind;

/// True for parentRef entries that attach the route to a Gateway listener
/// (the Gateway API default when group/kind are absent). Delegation
/// parentRefs (`kind: GRPCRoute`) return false.
fn is_gateway_parent_ref(pr: &ParentReference) -> bool {
    pr.canonical_identity(None).is_gateway()
}

/// True for parentRef entries that declare a parent GRPCRoute.
fn is_delegation_parent_ref(pr: &ParentReference) -> bool {
    let identity = pr.canonical_identity(None);
    identity.group == GATEWAY_API_GROUP && identity.kind == "GRPCRoute"
}

/// On creation of `route_key`, requeue the parents that delegate to it by
/// name: their `BackendNotFound` turns into `RefNotPermitted` or clears.
/// `on_change` runs before the save to cache, so a key missing from the cache
/// is a new route; updates do not requeue (a change of the declared parents
/// is requeued through the delegation graph). One index probe when no parent
/// names the route.
async fn requeue_parents_naming_on_create(route_key: &str, ctx: &HandlerContext) {
    let parents = ctx.runtime().grpc_route_delegation_graph.parents_naming(route_key);
    if parents.is_empty() || route_exists(ctx.runtime(), RouteKindName::GRPCRoute, route_key) {
        return;
    }
    for parent in parents {
        if parent != route_key {
            ctx.requeue("GRPCRoute", parent).await;
        }
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

    /// True when the route fails its own validation, so `update_status`
    /// rejects every one of its entries for a reason of its own: a
    /// validation error (including an invalid reference source), invalid
    /// hostnames, a route-wide plugin scope error, or no rule left to serve.
    /// Such a route gets no delegation level, so its descendants report
    /// `NotAttached` instead of `DelegationAccepted`.
    fn rejected_on_own_validation(&self, route: &GRPCRoute, ctx: &HandlerContext, facts: &GrpcDelegationFacts) -> bool {
        if !self.validate(route, ctx).is_empty()
            || route.spec.validate_hostnames().is_err()
            || !backend_route_wide_scope_errors(route, ctx.runtime()).is_empty()
        {
            return true;
        }
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let invalid = find_controller_invalid_rule_indices_with(route, route_ns, ctx.runtime(), facts);
        !invalid.is_empty() && invalid.len() == route.spec.rules.as_ref().map_or(0, Vec::len)
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
    /// Only the route's own authored backendRefs register: delegated children
    /// register their own dependencies.
    fn register_service_refs(route: &GRPCRoute, ctx: &HandlerContext) -> Vec<String> {
        let backend_refs_list: Vec<BackendDependencyReceipt> = GrpcRouteBackendRefs::from_route(route)
            .map(|references| route_backend_refs::collect_dependencies(references.authored()))
            .unwrap_or_default();

        super::route_utils::register_service_backend_refs(
            &Self::create_resource_ref(route),
            &backend_refs_list,
            ctx.runtime(),
        )
    }

    /// Register rule- and backend-level EdgionPlugins ExtensionRefs.
    ///
    /// Backend references are required so adding or removing WAF from a plugin
    /// generation revalidates the route's backend-scope ownership constraint.
    fn register_plugin_refs(route: &GRPCRoute, ctx: &HandlerContext) {
        let Ok(references) = GrpcRouteExtensionRefs::from_route(route) else {
            return;
        };
        let root = Self::create_resource_ref(route);
        // Parse entry clears the old generation; register every valid
        // dependency of the route's rules, including rules later withdrawn.
        for field in references.authored() {
            if let Some(receipt) = super::route_utils::ExtensionDependencyReceipt::from_field(field) {
                receipt.register(&root, ctx.runtime());
            }
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
    fn finalize_transport(&self, route: &mut GRPCRoute, _ctx: &HandlerContext) {
        route.spec.resolved_status_controller = Some(self.controller_name.clone());
    }

    fn validate(&self, route: &GRPCRoute, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = validate_grpc_route_if_enabled(route);
        // Note: backend ref validation (service existence, kind check) is handled by
        // update_status() via ResolvedRefs condition and per-rule invalid_rule_indices.
        if let Err(error) = GrpcRouteBackendRefs::from_route(route) {
            errors.push(route_backend_refs::source_error_message(error));
        }
        errors
    }

    async fn parse(&self, mut route: GRPCRoute, _ctx: &HandlerContext) -> ProcessResult<GRPCRoute> {
        // Controller-resolved delegation edges are never operator-authored.
        route.spec.resolved_delegation_keys = Vec::new();
        let source_valid = GrpcRouteBackendRefs::from_route(&route).is_ok();
        let grant_owner = Self::create_resource_ref(&route);
        _ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&grant_owner);
        _ctx.runtime().plugin_ref_mgr.clear_value_refs(&grant_owner);
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let route_ns = route_ns.as_str();
        if let Some(parents) = route.spec.parent_refs.as_deref().filter(|_| source_valid) {
            _ctx.runtime().gateway_route_index.register_route_dependencies(
                ResourceKind::GRPCRoute,
                &format!("{route_ns}/{}", grant_owner.name),
                parents,
                route_ns,
            );
        }

        // Authorize actual authored loans.
        let decisions = GrpcRouteBackendRefs::from_route(&route)
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

        // Register before protocol filtering so a plugin update can restore a
        // previously withdrawn rule.
        Self::register_plugin_refs(&route, _ctx);

        // Register Service backend references, then requeue BTPs bound to any
        // Service dropped from backendRefs (the apply path otherwise only
        // requeues BTPs for the route's current backends).
        let dropped_service_keys = Self::register_service_refs(&route, _ctx);
        requeue_backend_policies_for_services(&dropped_service_keys, _ctx).await;

        // Delegation graph (§5.2). An ordinary route costs one participant
        // probe; participants update their edges and requeue the routes whose
        // confirmation or level may have changed.
        let facts = grpc_delegation_facts(&route, _ctx.runtime());
        let gateway_attached = route.spec.parent_refs.iter().flatten().any(is_gateway_parent_ref);
        let graph = &_ctx.runtime().grpc_route_delegation_graph;
        let route_name = route.metadata.name.clone().unwrap_or_default();
        let own_rejected = || self.rejected_on_own_validation(&route, _ctx, &facts);
        if let Some(requeue) =
            sync_delegation_graph(graph, route_ns, &route_name, &facts, gateway_attached, own_rejected)
        {
            if matches!(facts.shape, ParentRefShape::Child(_)) && !facts.inert {
                route.spec.resolved_delegation_keys = resolve_child_keys(graph, &format!("{route_ns}/{route_name}"));
            }
            for key in requeue {
                _ctx.requeue("GRPCRoute", key).await;
            }
        }

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

        ProcessResult::Continue(route)
    }

    async fn on_change(&self, route: &GRPCRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        let dropped_gateways = update_gateway_route_index(
            ResourceKind::GRPCRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );
        for gateway_key in dropped_gateways {
            ctx.requeue("Gateway", gateway_key).await;
        }

        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        requeue_backend_policies_for_owner(&Self::create_resource_ref(route), ctx).await;

        // Delegation requeues (declared parents, target children, level
        // dependents) are issued by `parse` from the delegation graph. Parents
        // naming a newly created route refresh their `ResolvedRefs`, for
        // ordinary routes too.
        requeue_parents_naming_on_create(&format!("{route_ns}/{route_name}"), ctx).await;
    }

    async fn on_delete(&self, route: &GRPCRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_value_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        requeue_backend_policies_for_owner(&Self::create_resource_ref(route), ctx).await;
        super::route_utils::clear_service_backend_refs(&resource_ref, ctx.runtime());

        remove_from_gateway_route_index(ResourceKind::GRPCRoute, route_ns, route_name, ctx.runtime());

        // Delegation cleanup: a participant leaves the graph and requeues its
        // declared children and declared parents; parents naming this route
        // refresh their `BackendNotFound`. An ordinary route costs one
        // read-locked probe.
        let route_key = format!("{route_ns}/{route_name}");
        for key in ctx.runtime().grpc_route_delegation_graph.on_route_deleted(&route_key) {
            ctx.requeue("GRPCRoute", key).await;
        }

        let tracker_changed =
            remove_from_attached_route_tracker(ResourceKind::GRPCRoute, route_ns, route_name, ctx.runtime());
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_status(&self, route: &mut GRPCRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let source_error = GrpcRouteBackendRefs::from_route(route).err();
        if source_error.is_some() {
            _ctx.runtime()
                .plugin_ref_mgr
                .clear_value_refs(&Self::create_resource_ref(route));
            route.spec.resolved_listeners = None;
            route.spec.resolved_hostnames = None;
        }
        if source_error.is_none() {
            Self::register_plugin_refs(route, _ctx);
        }
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let hostname_error = route.spec.validate_hostnames().err();
        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        if let Some(error) = source_error {
            let message = route_backend_refs::source_error_message(error);
            if !validation_errors.contains(&message) {
                validation_accepted_errors.push(AcceptedError::Invalid { message });
            }
        }
        let plugin_scope_errors = if source_error.is_none() {
            backend_route_wide_scope_errors(route, _ctx.runtime())
        } else {
            Vec::new()
        };
        validation_accepted_errors.extend(plugin_scope_errors.clone());
        let plugin_scope_rejects_route = !plugin_scope_errors.is_empty();

        let mut resolved_refs_errors = route_backend_refs::validate_targets(
            GrpcRouteBackendRefs::from_route(route).map(|references| references.effective()),
            _ctx.runtime(),
        );
        if let Ok(refs) = GrpcRouteExtensionRefs::from_route(route) {
            resolved_refs_errors.extend(
                refs.effective()
                    .filter_map(|field| super::route_utils::ambiguous_plugin_ref(field, _ctx.runtime())),
            );
        }
        let ref_denied_list = collect_ref_denied_list(route);
        resolved_refs_errors.extend(super::route_utils::collect_ref_denied_errors(
            route_ns,
            ResourceKind::GRPCRoute,
            &ref_denied_list,
        ));

        // Delegation (§7): named-target errors of this route as a parent ride
        // ResolvedRefs; an illegal parentRef shape rejects every entry.
        let facts = grpc_delegation_facts(route, _ctx.runtime());
        let own_key = format!("{route_ns}/{}", route.metadata.name.as_deref().unwrap_or(""));
        let graph = &_ctx.runtime().grpc_route_delegation_graph;
        if source_error.is_none() {
            resolved_refs_errors.extend(parent_named_target_errors(
                graph,
                _ctx.runtime(),
                RouteKindName::GRPCRoute,
                &own_key,
                &facts,
            ));
        }
        let shape_error = match &facts.shape {
            ParentRefShape::Invalid(reason) => Some(AcceptedError::Delegation {
                reason: INVALID_PARENT_REFS,
                message: (*reason).to_string(),
            }),
            _ => None,
        };

        // Per-rule validation: find rules where ALL backend refs are invalid
        let invalid_indices = if source_error.is_none() {
            find_controller_invalid_rule_indices_with(route, route_ns, _ctx.runtime(), &facts)
        } else {
            Vec::new()
        };
        let total_rules = route.spec.rules.as_ref().map_or(0, |r| r.len());
        let valid_rules = total_rules - invalid_indices.len();
        let serves_no_rule = valid_rules == 0;
        // Static rule errors withdraw only the offending rule. Reject the
        // attachment when no effective rule survives, and retain valid siblings.
        let mut rule_errors: Vec<(usize, String)> = route
            .spec
            .rules
            .iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, rule)| {
                rule.method_match_error()
                    .or_else(|| rule.header_modifier_error())
                    .map(str::to_string)
                    .or_else(|| {
                        source_error
                            .is_none()
                            .then(|| backend_plugin_scope_invalid_reason(route, index, _ctx.runtime(), false))
                            .flatten()
                    })
                    .map(|message| (index, message))
            })
            .collect();
        // Malformed delegating rules and backend-selecting plugins on a
        // delegating rule ride the same invalid-rule path.
        if source_error.is_none() {
            for (index, reason) in facts.invalid_rules() {
                if !rule_errors.iter().any(|(existing, _)| existing == index) {
                    rule_errors.push((*index, reason.clone()));
                }
            }
            rule_errors.sort_by_key(|(index, _)| *index);
        }
        let attachment_rejects_route = serves_no_rule && !invalid_indices.is_empty();
        if attachment_rejects_route {
            validation_accepted_errors.extend(invalid_indices.iter().map(|index| AcceptedError::UnsupportedValue {
                message: format!("rules[{index}]: {}", rule_errors.iter()
                    .find(|(rule_index, _)| rule_index == index)
                    .map_or("invalid rule configuration", |(_, message)| message.as_str())),
            }));
        }
        let route_rejected = source_error.is_some()
            || hostname_error.is_some()
            || shape_error.is_some()
            || plugin_scope_rejects_route
            || attachment_rejects_route;

        route.spec.invalid_rule_indices = if invalid_indices.is_empty() {
            None
        } else {
            Some(invalid_indices.clone())
        };
        // Own dropped rules, listed on every accepted entry while an effective
        // rule remains matchable.
        let own_invalid = !invalid_indices.is_empty() && valid_rules > 0;
        let mut own_invalid_reasons: Vec<String> = Vec::new();
        if own_invalid {
            own_invalid_reasons.push(format!(
                "Rules at indices {:?} are invalid; {} of {} rules are active",
                invalid_indices, valid_rules, total_rules
            ));
            own_invalid_reasons.extend(
                rule_errors
                    .iter()
                    .map(|(index, message)| format!("rules[{index}]: {message}")),
            );
        }

        let status = route.status.get_or_insert_with(|| GRPCRouteStatus { parents: vec![] });
        let route_hostnames = route.spec.hostnames.as_ref();

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                // Delegation parentRefs (kind: GRPCRoute) get their own
                // Accepted semantics below, not Gateway acceptance.
                if !is_gateway_parent_ref(parent_ref) {
                    continue;
                }
                let mut accepted_errors = super::route_utils::gateway_parent_status_errors(
                    source_error.is_none().then_some(route_ns),
                    parent_ref,
                    route_hostnames,
                    "GRPCRoute",
                    _ctx.runtime(),
                );
                if let Some(error) = hostname_error {
                    accepted_errors.insert(
                        0,
                        AcceptedError::UnsupportedValue {
                            message: error.to_string(),
                        },
                    );
                }
                if let Some(error) = &shape_error {
                    accepted_errors.insert(0, error.clone());
                }
                accepted_errors.extend(validation_accepted_errors.clone());

                // Ordinary backend resolution errors do not affect Accepted.
                // Backend-scoped WAF is different: it cannot join the
                // request-wide ownership election and rejects the whole route.

                let ps = super::route_utils::parent_status_for_update(
                    &mut status.parents,
                    parent_ref,
                    route_ns,
                    &self.controller_name,
                );
                let conditions = &mut ps.conditions;
                set_parent_conditions_full(conditions, &accepted_errors, &resolved_refs_errors, generation);
                let reasons = if route_rejected {
                    &[][..]
                } else {
                    &own_invalid_reasons[..]
                };
                set_partially_invalid(conditions, reasons, None, generation);
            }

            // Child entries (§7.2): one per `kind: GRPCRoute` parentRef.
            for parent_ref in parent_refs.iter().filter(|pr| is_delegation_parent_ref(pr)) {
                let parent_key = format!(
                    "{}/{}",
                    parent_ref.namespace.as_deref().unwrap_or(route_ns),
                    parent_ref.name
                );
                let mut accepted_errors = Vec::new();
                let mut accepted_message = String::new();
                let mut conflicts = None;
                if source_error.is_some() {
                    accepted_errors.push(AcceptedError::Invalid {
                        message: "Invalid route reference source".to_string(),
                    });
                } else {
                    let rules = &route.spec.rules;
                    let child_rules = || grpc_child_rules(rules.as_ref());
                    match child_entry_verdict(
                        graph,
                        RouteKindName::GRPCRoute,
                        &own_key,
                        &parent_key,
                        &facts,
                        child_rules,
                    ) {
                        ChildEntryVerdict::Accepted {
                            message,
                            conflicting_rules,
                            out_of_scope_rules,
                        } => {
                            accepted_message = message;
                            conflicts = edge_message(
                                RouteKindName::GRPCRoute,
                                &parent_key,
                                &conflicting_rules,
                                &out_of_scope_rules,
                            );
                        }
                        ChildEntryVerdict::Rejected { error } => accepted_errors.push(error),
                    }
                }
                if let Some(error) = hostname_error {
                    accepted_errors.push(AcceptedError::UnsupportedValue {
                        message: error.to_string(),
                    });
                }
                accepted_errors.extend(validation_accepted_errors.clone());

                // Match on kind too: a route can carry both a `kind: Gateway`
                // and a `kind: GRPCRoute` parentRef with the same name/ns/
                // section, and the delegation entry must not overwrite the
                // Gateway-acceptance entry.
                let ps = super::route_utils::parent_status_for_update(
                    &mut status.parents,
                    parent_ref,
                    route_ns,
                    &self.controller_name,
                );
                let conditions = &mut ps.conditions;
                set_parent_conditions_full(conditions, &accepted_errors, &resolved_refs_errors, generation);
                let mut reasons = Vec::new();
                let mut conflict = None;
                if accepted_errors.is_empty() {
                    update_condition(
                        conditions,
                        condition_true(
                            condition_types::ACCEPTED,
                            DELEGATION_ACCEPTED,
                            accepted_message,
                            generation,
                        ),
                    );
                    if !route_rejected {
                        reasons.extend(own_invalid_reasons.iter().cloned());
                        conflict = conflicts;
                    }
                }
                set_partially_invalid(conditions, &reasons, conflict, generation);
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
            ResourceKind::GRPCRoute,
            route_ns,
            route.metadata.name.as_deref().unwrap_or(""),
            route
                .spec
                .parent_refs
                .as_ref()
                .filter(|_| source_error.is_none() && hostname_error.is_none() && shape_error.is_none()),
            route.spec.hostnames.as_ref(),
            _ctx.runtime(),
        );
        super::policy_ancestor_admission::annotate(route, &self.controller_name, _ctx);
    }
}

/// Collect Option<&RefDenied> from all backend refs for shared error collection.
///
/// Scans the route's own `spec.rules`.
fn collect_ref_denied_list(route: &GRPCRoute) -> Vec<Option<&RefDenied>> {
    let mut list = Vec::new();
    for rule in route.spec.rules.iter().flatten() {
        if let Some(backend_refs) = &rule.backend_refs {
            for br in backend_refs {
                list.push(br.ref_denied.as_ref());
            }
        }
    }
    list
}

/// Set `PartiallyInvalid=True` listing the route's own dropped rules
/// (`Dropped Rule(s):`) and this entry's edge-conflict segment, or remove it:
/// it is a negative-polarity Gateway API condition and must be absent unless
/// True.
fn set_partially_invalid(
    conditions: &mut Vec<edgion_resources::resources::common::Condition>,
    reasons: &[String],
    conflict: Option<String>,
    generation: Option<i64>,
) {
    let mut segments = Vec::new();
    if !reasons.is_empty() {
        segments.push(format!("Dropped Rule(s): {}", reasons.join("; ")));
    }
    segments.extend(conflict);
    if segments.is_empty() {
        conditions.retain(|condition| condition.type_ != condition_types::PARTIALLY_INVALID);
        return;
    }
    update_condition(
        conditions,
        condition_true(
            condition_types::PARTIALLY_INVALID,
            condition_reasons::RULE_INVALID,
            segments.join("; "),
            generation,
        ),
    );
}

/// True if a filter violates the `type`/`extensionRef` union or targets an
/// unsupported extension.
///
/// `ExtensionRef` has exactly one recognized use: `EdgionPlugins` in the
/// `edgion.io` API group. An empty group denotes the Kubernetes core API group.
/// The former route-level
/// extensionRef-based load-balancing-policy mechanism has been removed; those
/// kinds are now flagged like any other unsupported kind. Load-balancing
/// policy is configured via `EdgionBackendTrafficPolicy`.
///
/// Anything else is a static config error; the gRPC side has no reason-string
/// plumbing, so this only contributes the rule index to `invalid_rule_indices`.
fn has_unsupported_extension_ref(filters: Option<&Vec<GRPCRouteFilter>>) -> bool {
    let Some(filters) = filters else { return false };
    filters.iter().any(|filter| {
        validate_extension_ref_filter(
            filter.filter_type == GRPCRouteFilterType::ExtensionRef,
            filter.extension_ref.as_ref(),
        )
        .is_err()
    })
}

/// Find rule indices where the rule is invalid.
///
/// A rule is invalid if any method predicate is invalid, or it (or any of its backend refs) has a malformed or repeated
/// header modifier within a single filter list. Rules with invalid backends (non-Service kind
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
            if rule.method_match_error().is_some() || rule.header_modifier_error().is_some() {
                invalid_indices.push(idx);
                continue;
            }
            if has_unsupported_extension_ref(rule.filters.as_ref()) {
                invalid_indices.push(idx);
                continue;
            }
            // Invalid sessionPersistence (bad GEP-2257 absoluteTimeout, or
            // Permanent lifetime without absoluteTimeout). K8s admission already
            // rejects these; this guards FileSystem/Etcd modes so the rule is
            // dropped rather than silently emitting a never-expiring token.
            if rule
                .session_persistence
                .as_ref()
                .is_some_and(|sp| sp.validate().is_err())
            {
                invalid_indices.push(idx);
                continue;
            }
            if let Some(backend_refs) = &rule.backend_refs {
                let backend_conflict = backend_refs
                    .iter()
                    .any(|br| has_unsupported_extension_ref(br.filters.as_ref()));
                if backend_conflict {
                    invalid_indices.push(idx);
                }
            }
        }
    }
    invalid_indices
}

fn find_controller_invalid_rule_indices_with(
    route: &GRPCRoute,
    route_namespace: &str,
    runtime: &ControllerRuntime,
    facts: &GrpcDelegationFacts,
) -> Vec<usize> {
    let mut invalid = find_invalid_rule_indices(route, route_namespace);
    // Malformed delegating rules and backend-selecting plugins on a delegating
    // rule ride the same invalid-rule path.
    for (index, _) in facts.invalid_rules() {
        if !invalid.contains(index) {
            invalid.push(*index);
        }
    }
    let already_invalid: HashSet<usize> = invalid.iter().copied().collect();
    for (index, _) in route.spec.rules.iter().flatten().enumerate() {
        if !already_invalid.contains(&index)
            && backend_plugin_scope_invalid_reason(route, index, runtime, false).is_some()
        {
            invalid.push(index);
        }
    }
    invalid.sort_unstable();
    invalid
}

fn backend_plugin_scope_invalid_reason(
    route: &GRPCRoute,
    rule_index: usize,
    runtime: &ControllerRuntime,
    route_wide_only: bool,
) -> Option<String> {
    let references = GrpcRouteExtensionRefs::from_route(route).ok()?;
    let root = GrpcRouteHandler::create_resource_ref(route);
    let fields = || references.authored().filter(|field| field.slot().rule() == rule_index);
    // Register the complete valid set before any applicable content inspection.
    for field in fields() {
        if let Some(receipt) = super::route_utils::ExtensionDependencyReceipt::from_field(field) {
            receipt.register(&root, runtime);
        }
    }
    if find_invalid_rule_indices(route, route.metadata.namespace.as_deref().unwrap_or("default")).contains(&rule_index)
    {
        return None;
    }
    // Rule-level transcoder rejection precedes backend-scope WAF/RequestId,
    // matching the existing gRPC status priority.
    if let Some(plugin) = fields()
        .filter(|field| field.slot().backend().is_none())
        .find_map(|field| super::route_utils::http_route_only_plugin(runtime, &root, field))
    {
        return Some(format!("{plugin} requires HTTPRoute rule scope"));
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

fn backend_route_wide_scope_errors(route: &GRPCRoute, runtime: &ControllerRuntime) -> Vec<AcceptedError> {
    let invalid = find_invalid_rule_indices(route, route.metadata.namespace.as_deref().unwrap_or("default"))
        .into_iter()
        .collect::<HashSet<_>>();
    route
        .spec
        .rules
        .iter()
        .flatten()
        .enumerate()
        .filter(|(index, _)| !invalid.contains(index))
        .filter_map(|(index, _)| {
            backend_plugin_scope_invalid_reason(route, index, runtime, true).map(|message| {
                AcceptedError::UnsupportedValue {
                    message: format!("rules[{index}]: {message}"),
                }
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route_test/delegation_status_test.rs"]
mod delegation_status_test;

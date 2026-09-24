//! GRPCRoute Handler
//!
//! Handles GRPCRoute resources with ReferenceGrant validation, cross-namespace
//! reference tracking, and GRPCRoute-to-GRPCRoute delegation flattening.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::{
    is_cross_ns_ref_allowed, read_after_registering_cross_ns_dependency, validate_grpc_route_if_enabled,
};
use super::grpc_route_delegation::{flatten_route, DelegationLookup};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_policies_for_route,
    requeue_backend_policies_for_services, requeue_parent_gateways, update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, set_parent_conditions_full, update_condition,
    AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::GRPCRoute;
use edgion_resources::resources::common::{Condition, ParentReference, RefDenied};
use edgion_resources::resources::extension_ref::{is_supported_edgion_plugins_ref, validate_extension_ref_filter};
use edgion_resources::resources::grpc_route::{GRPCRouteFilter, GRPCRouteFilterType, GRPCRouteStatus};
use edgion_resources::resources::http_route::{DelegationIssue, GATEWAY_API_GROUP};
use edgion_resources::ResourceKind;

/// True for parentRef entries that attach the route to a Gateway listener
/// (the Gateway API default when group/kind are absent). Delegation
/// parentRefs (`kind: GRPCRoute`, reverse-lock declarations) return false.
fn is_gateway_parent_ref(pr: &ParentReference) -> bool {
    pr.canonical_identity(None).is_gateway()
}

/// True for parentRef entries that reverse-lock delegation to specific parent
/// GRPCRoutes.
fn is_delegation_parent_ref(pr: &ParentReference) -> bool {
    let identity = pr.canonical_identity(None);
    identity.group == GATEWAY_API_GROUP && identity.kind == "GRPCRoute"
}

/// `DelegationLookup` backed by the live controller stores.
struct RuntimeDelegationLookup<'a> {
    runtime: &'a ControllerRuntime,
    grant_owner: Option<ResourceRef>,
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
        let read = || {
            is_cross_ns_ref_allowed(
                from_ns,
                "GRPCRoute",
                to_ns,
                group.as_ref(),
                kind.as_ref(),
                name,
                self.runtime,
            )
        };
        self.grant_owner.as_ref().map_or_else(read, |owner| {
            read_after_registering_cross_ns_dependency(self.runtime, to_ns, owner, read)
        })
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
    ) -> Vec<String> {
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
        )
    }

    /// Record cross-namespace references from backend_refs.
    ///
    /// `extra_namespaces` carries the cross-namespace targets of flattened
    /// delegation backendRefs so ReferenceGrant changes in those namespaces
    /// requeue this parent route. Parse entry replaces the resource's whole
    /// ref set; this later pass only extends it with authored and flattened targets.
    fn record_cross_ns_refs(route: &GRPCRoute, extra_namespaces: &HashSet<String>, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().cross_ns_ref_mgr;

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

    /// Register rule- and backend-level EdgionPlugins ExtensionRefs.
    ///
    /// Backend references are required so adding or removing WAF from a plugin
    /// generation revalidates the route's backend-scope ownership constraint.
    fn register_plugin_refs(route: &GRPCRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().plugin_ref_mgr;
        manager.clear_value_refs(&resource_ref);

        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        for rules in [route.spec.rules.as_deref(), route.spec.resolved_rules.as_deref()]
            .into_iter()
            .flatten()
        {
            for rule in rules {
                let filters = rule.filters.iter().flatten().chain(
                    rule.backend_refs
                        .iter()
                        .flatten()
                        .flat_map(|backend_ref| backend_ref.filters.iter().flatten()),
                );
                for filter in filters {
                    if filter.filter_type != GRPCRouteFilterType::ExtensionRef {
                        continue;
                    }
                    let Some(extension_ref) = &filter.extension_ref else {
                        continue;
                    };
                    if is_supported_edgion_plugins_ref(extension_ref) {
                        let ref_namespace = extension_ref.resolved_namespace.as_deref().unwrap_or(namespace);
                        manager.add_ref(format!("{ref_namespace}/{}", extension_ref.name), resource_ref.clone());
                    }
                }
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
        let errors = validate_grpc_route_if_enabled(route);
        // Note: backend ref validation (service existence, kind check) is handled by
        // update_status() via ResolvedRefs condition and per-rule invalid_rule_indices.
        errors
    }

    async fn parse(&self, mut route: GRPCRoute, _ctx: &HandlerContext) -> ProcessResult<GRPCRoute> {
        let grant_owner = Self::create_resource_ref(&route);
        _ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&grant_owner);
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let route_ns = route_ns.as_str();

        // `resolvedNamespace` is controller-owned. Clear any filesystem/etcd
        // injection before the delegation flattener stamps derived copies.
        for rule in route.spec.rules.iter_mut().flatten() {
            for filter in rule.filters.iter_mut().flatten().chain(
                rule.backend_refs
                    .iter_mut()
                    .flatten()
                    .flat_map(|backend_ref| backend_ref.filters.iter_mut().flatten()),
            ) {
                if filter.filter_type == GRPCRouteFilterType::ExtensionRef {
                    if let Some(extension_ref) = filter.extension_ref.as_mut() {
                        extension_ref.resolved_namespace = None;
                    }
                }
            }
        }

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
                                let allowed = read_after_registering_cross_ns_dependency(
                                    _ctx.runtime(),
                                    backend_ns,
                                    &grant_owner,
                                    || {
                                        is_cross_ns_ref_allowed(
                                            route_ns,
                                            "GRPCRoute",
                                            backend_ns,
                                            backend_ref.group.as_ref(),
                                            backend_ref.kind.as_ref(),
                                            &backend_ref.name,
                                            _ctx.runtime(),
                                        )
                                    },
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
        let mut delegation_outcome = {
            let adapter = RuntimeDelegationLookup {
                runtime: _ctx.runtime(),
                grant_owner: Some(grant_owner.clone()),
            };
            flatten_route(&route, &adapter)
        };
        route.spec.resolved_rules = delegation_outcome.resolved_rules.clone();

        // Register before filtering so a plugin update that removes WAF can
        // restore a previously withdrawn delegated rule.
        Self::register_plugin_refs(&route, _ctx);
        if let Some(rules) = &mut route.spec.resolved_rules {
            let mut resolved_index = 0usize;
            rules.retain(|rule| {
                let reason = backend_plugin_scope_invalid_reason(rule, route_ns, _ctx.runtime(), false);
                let keep = reason.is_none();
                if let Some(reason) = reason {
                    delegation_outcome.issues.push(DelegationIssue::RuleDropped {
                        message: format!("resolvedRules[{resolved_index}]: {reason}"),
                    });
                }
                resolved_index += 1;
                keep
            });
            delegation_outcome.resolved_rules = route.spec.resolved_rules.clone();
        }

        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route, &delegation_outcome.cross_ns_target_namespaces, _ctx);

        // Register Service backend references for cross-resource requeue
        // Register Service backend references, then requeue BTPs bound to any
        // Service dropped from backendRefs (the apply path otherwise only
        // requeues BTPs for the route's current backends).
        let dropped_service_keys =
            Self::register_service_refs(&route, &delegation_outcome.service_backend_tuples, _ctx);
        requeue_backend_policies_for_services(&dropped_service_keys, _ctx).await;

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
                    grant_owner: None,
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
                    grant_owner: None,
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
        route.spec.resolved_hostnames = None;
        if let Some(annotations) = route.metadata.annotations.as_mut() {
            annotations.remove("edgion.io/hostname-resolution");
        }
        let valid_hostnames = route.spec.validate_hostnames().is_ok();

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
        requeue_backend_policies_for_route(ResourceKind::GRPCRoute, route_ns, route_name, ctx).await;

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
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_value_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        requeue_backend_policies_for_route(ResourceKind::GRPCRoute, route_ns, route_name, ctx).await;
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
            let adapter = RuntimeDelegationLookup {
                runtime: ctx.runtime(),
                grant_owner: None,
            };
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

        let hostname_error = route.spec.validate_hostnames().err();
        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        let plugin_scope_errors = backend_route_wide_scope_errors(route, route_ns, _ctx.runtime());
        validation_accepted_errors.extend(plugin_scope_errors.clone());
        let plugin_scope_rejects_route = !plugin_scope_errors.is_empty();

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
        let invalid_indices = find_controller_invalid_rule_indices(route, route_ns, _ctx.runtime());
        let total_rules = route.spec.rules.as_ref().map_or(0, |r| r.len());
        let valid_rules = total_rules - invalid_indices.len();
        let serves_no_rule = match &route.spec.resolved_rules {
            Some(resolved) => resolved.is_empty(),
            None => valid_rules == 0,
        };
        // Static rule errors withdraw only the offending rule. Reject the
        // attachment when no effective rule survives, and retain valid siblings.
        let rule_errors: Vec<(usize, String)> = route
            .spec
            .rules
            .iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, rule)| {
                rule.method_match_error()
                    .or_else(|| rule.header_modifier_error())
                    .map(str::to_string)
                    .or_else(|| backend_plugin_scope_invalid_reason(rule, route_ns, _ctx.runtime(), false))
                    .map(|message| (index, message))
            })
            .collect();
        let attachment_rejects_route =
            serves_no_rule && (!invalid_indices.is_empty() || !delegation_dropped.is_empty());
        if attachment_rejects_route {
            validation_accepted_errors.extend(invalid_indices.iter().map(|index| AcceptedError::UnsupportedValue {
                message: format!("rules[{index}]: {}", rule_errors.iter()
                    .find(|(rule_index, _)| rule_index == index)
                    .map_or("invalid rule configuration", |(_, message)| message.as_str())),
            }));
            validation_accepted_errors.extend(
                delegation_dropped
                    .iter()
                    .cloned()
                    .map(|message| AcceptedError::UnsupportedValue { message }),
            );
        }
        let route_rejected = hostname_error.is_some() || plugin_scope_rejects_route || attachment_rejects_route;

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
                if let Some(error) = hostname_error {
                    accepted_errors.insert(
                        0,
                        AcceptedError::UnsupportedValue {
                            message: error.to_string(),
                        },
                    );
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

                // Set PartiallyInvalid condition (own-rule findings plus
                // delegation drops — a dropped delegated rule never flips
                // Accepted, matching the per-rule precedent above).
                let own_invalid = !invalid_indices.is_empty() && valid_rules > 0;
                if !route_rejected && (own_invalid || !delegation_dropped.is_empty()) {
                    let mut reasons: Vec<String> = Vec::new();
                    if own_invalid {
                        reasons.push(format!(
                            "Rules at indices {:?} are invalid; {} of {} rules are active",
                            invalid_indices, valid_rules, total_rules
                        ));
                        reasons.extend(
                            rule_errors
                                .iter()
                                .map(|(index, message)| format!("rules[{index}]: {message}")),
                        );
                    }
                    reasons.extend(delegation_dropped.iter().cloned());
                    let message = format!("Dropped Rule(s): {}", reasons.join("; "));
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
            ResourceKind::GRPCRoute,
            route_ns,
            route.metadata.name.as_deref().unwrap_or(""),
            route.spec.parent_refs.as_ref().filter(|_| hostname_error.is_none()),
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
fn collect_backend_ref_tuples(route: &GRPCRoute) -> Vec<super::route_utils::BackendRefTuple<'_>> {
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

fn find_controller_invalid_rule_indices(
    route: &GRPCRoute,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Vec<usize> {
    let mut invalid = find_invalid_rule_indices(route, route_namespace);
    let already_invalid: HashSet<usize> = invalid.iter().copied().collect();
    for (index, rule) in route.spec.rules.iter().flatten().enumerate() {
        if !already_invalid.contains(&index)
            && backend_plugin_scope_invalid_reason(rule, route_namespace, runtime, false).is_some()
        {
            invalid.push(index);
        }
    }
    invalid.sort_unstable();
    invalid
}

fn backend_plugin_scope_invalid_reason(
    rule: &edgion_resources::resources::grpc_route::GRPCRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
    route_wide_only: bool,
) -> Option<String> {
    rule.backend_refs
        .iter()
        .flatten()
        .enumerate()
        .find_map(|(index, backend_ref)| {
            backend_ref.filters.iter().flatten().find_map(|filter| {
                if filter.filter_type != GRPCRouteFilterType::ExtensionRef {
                    return None;
                }
                filter.extension_ref.as_ref().and_then(|extension_ref| {
                    if route_wide_only {
                        super::route_utils::backend_request_scope_route_wide_error(
                            runtime,
                            route_namespace,
                            index,
                            extension_ref,
                        )
                    } else {
                        super::route_utils::backend_request_scope_extension_ref_error(
                            runtime,
                            route_namespace,
                            index,
                            extension_ref,
                        )
                    }
                })
            })
        })
}

fn backend_route_wide_scope_errors(
    route: &GRPCRoute,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Vec<AcceptedError> {
    route
        .spec
        .rules
        .iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, rule)| {
            backend_plugin_scope_invalid_reason(rule, route_namespace, runtime, true).map(|message| {
                AcceptedError::UnsupportedValue {
                    message: format!("rules[{index}]: {message}"),
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn hostname_validation_clears_stale_attachments_and_recovers() {
        use super::super::GatewayHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::Gateway;

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
        raw.upsert(
            "apps/gateway".into(),
            Arc::new(
                serde_json::from_value(serde_json::json!({
                    "metadata": {"namespace": "apps", "name": "gateway"},
                    "spec": {"gatewayClassName": "test", "listeners": [
                        {"name": "http", "port": 8080, "protocol": "HTTP"}
                    ]}
                }))
                .unwrap(),
            ),
        );
        let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
        let handler = GrpcRouteHandler::default();
        let mut route: GRPCRoute = serde_json::from_value(serde_json::json!({
            "metadata": {"namespace": "apps", "name": "route"},
            "spec": {"parentRefs": [{"name": "gateway", "sectionName": "http"}], "rules": [{}]}
        }))
        .unwrap();
        for (hostnames, accepted) in [
            (None, true),
            (Some(Vec::new()), true),
            (Some(vec!["a.example.com".to_string(); 16]), true),
            (Some(vec!["a.example.com".to_string(); 17]), false),
            (Some(vec!["a.example.com".to_string()]), true),
            (Some(vec!["a.example.com".to_string(), "*".to_string()]), false),
            (Some(vec!["127.0.0.1".to_string()]), false),
            (Some(vec!["*.127.0.0.1".to_string()]), true),
            (Some(vec!["A.example.com".to_string()]), false),
            (Some(vec!["a.example.com".to_string()]), true),
        ] {
            route.spec.hostnames = hostnames;
            let errors = handler.validate(&route, &ctx);
            let ProcessResult::Continue(mut parsed) = handler.parse(route, &ctx).await else {
                panic!("invalid hostnames must retain the object for status reporting");
            };
            if !accepted {
                // A prior rule-level warning must not survive whole-route rejection.
                parsed.status.as_mut().unwrap().parents[0]
                    .conditions
                    .push(condition_true(
                        condition_types::PARTIALLY_INVALID,
                        condition_reasons::RULE_INVALID,
                        "Previous generation contained an invalid rule",
                        None,
                    ));
            }
            handler.update_status(&mut parsed, &ctx, &errors);
            let conditions = &parsed.status.as_ref().unwrap().parents[0].conditions;
            let condition = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
            assert_eq!(condition.status, if accepted { "True" } else { "False" });
            assert_eq!(condition.reason, if accepted { "Accepted" } else { "UnsupportedValue" });
            assert_eq!(parsed.spec.resolved_listeners.is_some(), accepted);
            assert_eq!(parsed.spec.resolved_hostnames.is_some(), accepted);
            if !accepted {
                assert!(parsed
                    .metadata
                    .annotations
                    .as_ref()
                    .is_none_or(|a| !a.contains_key("edgion.io/hostname-resolution")));
                assert!(!conditions
                    .iter()
                    .any(|c| c.type_ == "PartiallyInvalid" && c.status == "True"));
            }
            assert_eq!(
                runtime
                    .attached_route_tracker
                    .count_for_listener("apps/gateway", "http"),
                i32::from(accepted)
            );
            route = parsed;
        }
        route.spec.parent_refs = None;
        let ProcessResult::Continue(mut parsed) = handler.parse(route, &ctx).await else {
            panic!("parentless route must remain available for status reporting");
        };
        handler.update_status(&mut parsed, &ctx, &[]);
        assert!(parsed.spec.resolved_listeners.is_none());
        assert!(parsed.spec.resolved_hostnames.is_none());
        assert_eq!(
            runtime
                .attached_route_tracker
                .count_for_listener("apps/gateway", "http"),
            0
        );
    }

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

    fn make_ctx_with_raw_waf(namespace: &str, name: &str) -> HandlerContext {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::prelude_resources::EdgionPlugins;

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::new()),
            runtime.clone(),
        ));
        let plugins: EdgionPlugins = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionPlugins",
            "metadata": {"namespace": namespace, "name": name},
            "spec": {"waf": {
                "rules": "SecAction \\\"id:1,phase:1,pass\\\""
            }}
        }))
        .expect("WAF fixture must deserialize");
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert(format!("{namespace}/{name}"), Arc::new(plugins));
        processor.set_raw_store(raw);
        runtime.processor_registry.register(processor);
        HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
    }

    #[tokio::test]
    async fn backend_waf_is_controller_invalid_and_registered_for_requeue() {
        let ctx = make_ctx_with_raw_waf("apps", "waf");
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {"namespace": "apps", "name": "route"},
            "spec": {"rules": [{
                "backendRefs": [{
                    "name": "service", "port": 80,
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": {
                            "group": "edgion.io", "kind": "EdgionPlugins", "name": "waf"
                        }
                    }]
                }]
            }]}
        }))
        .expect("GRPCRoute fixture must deserialize");

        assert_eq!(
            find_controller_invalid_rule_indices(&route, "apps", ctx.runtime()),
            vec![0]
        );
        assert!(backend_plugin_scope_invalid_reason(
            &route.spec.rules.as_ref().unwrap()[0],
            "apps",
            ctx.runtime(),
            false
        )
        .unwrap()
        .contains("cannot contain WAF"));
        let accepted = backend_route_wide_scope_errors(&route, "apps", ctx.runtime());
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].reason(), condition_reasons::UNSUPPORTED_VALUE);
        assert!(accepted[0].message().contains("rules[0]"));

        GrpcRouteHandler::register_plugin_refs(&route, &ctx);
        let route_ref = GrpcRouteHandler::create_resource_ref(&route);
        assert_eq!(
            ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key()),
            vec!["apps/waf".to_string()]
        );
    }

    #[tokio::test]
    async fn backend_jump_invalidates_only_its_rule() {
        use super::super::GatewayHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::prelude_resources::EdgionPlugins;
        use edgion_resources::resources::Gateway;

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        let plugins: EdgionPlugins = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
            "metadata": {"namespace": "default", "name": "jump"},
            "spec": {"requestPlugins": [{"type": "RegionRoute", "config": {}}]}
        }))
        .unwrap();
        raw.upsert("default/jump".to_string(), Arc::new(plugins));
        processor.set_raw_store(raw);
        runtime.processor_registry.register(processor);
        let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            16,
            Arc::new(GatewayHandler::new(None, None)),
            runtime.clone(),
        ));
        let gateway_raw = Arc::new(LocalRawStore::new());
        let gateway: Gateway = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"namespace": "default", "name": "gw"},
            "spec": {"gatewayClassName": "edgion", "listeners": [{"name": "grpc", "protocol": "HTTP", "port": 80}]}
        }))
        .unwrap();
        gateway_raw.upsert("default/gw".to_string(), Arc::new(gateway));
        gateway_processor.set_raw_store(gateway_raw);
        runtime.processor_registry.register(gateway_processor);
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime);
        let mut route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
            "metadata": {"namespace": "default", "name": "jump-scope"},
            "spec": {"parentRefs": [{"name": "gw"}], "rules": [
                {"backendRefs": [{"name": "bad", "port": 80, "filters": [{"type": "ExtensionRef",
                    "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "jump"}}]}]},
                {"backendRefs": [{"name": "good", "port": 80}]}
            ]}
        }))
        .unwrap();
        GrpcRouteHandler::default().update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"
            && condition.status == "True"
            && condition.message.contains("RegionRoute")));
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == "Accepted" && condition.status == "True"));
    }

    #[tokio::test]
    async fn parse_clears_authored_extension_ref_namespace_metadata() {
        let ctx = make_ctx();
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {"namespace": "apps", "name": "route"},
            "spec": {"rules": [{
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "rule", "resolvedNamespace": "injected"
                }}],
                "backendRefs": [{"name": "service", "port": 80, "filters": [{
                    "type": "ExtensionRef", "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins", "name": "backend", "resolvedNamespace": "injected"
                    }
                }]}]
            }]}
        }))
        .expect("GRPCRoute fixture must deserialize");

        let parsed = match GrpcRouteHandler::default().parse(route, &ctx).await {
            ProcessResult::Continue(route) => route,
            other => panic!("unexpected parse result: {other:?}"),
        };
        let rule = &parsed.spec.rules.as_ref().unwrap()[0];
        assert!(rule.filters.as_ref().unwrap()[0]
            .extension_ref
            .as_ref()
            .unwrap()
            .resolved_namespace
            .is_none());
        assert!(rule.backend_refs.as_ref().unwrap()[0].filters.as_ref().unwrap()[0]
            .extension_ref
            .as_ref()
            .unwrap()
            .resolved_namespace
            .is_none());
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

    #[tokio::test]
    async fn method_validation_preserves_siblings_publishes_indices_and_recovers() {
        use super::super::GatewayHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::Gateway;

        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());
        let processor = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            16,
            Arc::new(GatewayHandler::new(None, None)),
            runtime,
        ));
        let raw = Arc::new(LocalRawStore::new());
        let gateway: Gateway = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"name": "gw", "namespace": "default"},
            "spec": {"gatewayClassName": "edgion", "listeners": [
                {"name": "grpc", "protocol": "HTTP", "port": 80}
            ]}
        }))
        .unwrap();
        raw.upsert("default/gw".to_string(), Arc::new(gateway));
        processor.set_raw_store(raw);
        ctx.runtime().processor_registry.register(processor);

        let mut route = route_with_rules(json!([
            {"matches": [{"method": {"method": "Get"}},
                {"method": {"type": "RegularExpression", "service": "private-marker[", "method": "Get"}}]},
            {"matches": [{"method": {}}]},
            {"matches": [{"method": {"method": "Get"}}]},
            {"matches": [{"method": {"type": "RegularExpression", "method": "^List.*$"}}]}
        ]));
        route.spec.parent_refs = Some(serde_json::from_value(json!([{"name": "gw"}])).unwrap());
        let handler = GrpcRouteHandler::default();
        let errors = validate_grpc_route_if_enabled(&route);
        assert!(errors.is_empty(), "per-rule errors must not reject valid siblings");
        route = match handler.parse(route, &ctx).await {
            ProcessResult::Continue(route) => route,
            other => panic!("unexpected parse result: {other:?}"),
        };
        handler.update_status(&mut route, &ctx, &errors);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|c| c.type_ == "Accepted" && c.status == "True"));
        assert!(conditions.iter().any(|c| c.type_ == "PartiallyInvalid"
            && c.status == "True"
            && c.message.contains("invalid regular expression")));
        assert!(!conditions.iter().any(|c| c.message.contains("private-marker")));
        let published: GRPCRoute = serde_json::from_slice(&serde_json::to_vec(&route).unwrap()).unwrap();
        assert_eq!(published.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
        assert_eq!(published.effective_rules().unwrap().len(), 4);
        assert!(published.effective_rules().unwrap()[2].method_match_error().is_none());

        route.spec.rules.as_mut().unwrap().truncate(2);
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|c| c.type_ == "Accepted"
            && c.status == "False"
            && c.reason == "UnsupportedValue"
            && c.message.contains("rules[0]")));
        assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

        for rule in route.spec.rules.iter_mut().flatten() {
            rule.matches = None;
        }
        handler.update_status(&mut route, &ctx, &[]);
        assert!(route.spec.invalid_rule_indices.is_none());
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|c| c.type_ == "Accepted" && c.status == "True"));
        assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

        // Delegation can leave no effective rules even when its authored
        // parent service scope is valid. That attachment must also reject.
        route.spec.resolved_rules = Some(vec![]);
        route.spec.delegation_issues = vec![DelegationIssue::RuleDropped {
            message: "child rule dropped: invalid method matcher".to_string(),
        }];
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions
            .iter()
            .any(|c| c.type_ == "Accepted" && c.status == "False" && c.reason == "UnsupportedValue"));
        assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));
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
    fn response_header_policy_rejects_backend_mutation_and_preserves_request_direction() {
        let mut route = route_with_rules(json!([
            {"backendRefs": [{"name": "svc", "port": 80, "filters": [
                {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"remove": ["Content-Length"]}}
            ]}]},
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "Content-Length", "value": "123"}]
            }}]}
        ]));
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
        route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].filters = None;
        assert!(super::find_invalid_rule_indices(&route, "default").is_empty());
    }

    #[test]
    fn header_modifier_status_preserves_siblings_rejects_empty_route_and_recovers() {
        let mut route = route_with_rules(json!([
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "x-test", "value": "secret-marker\r\ninvalid"}]
            }}]},
            {"backendRefs": [{"name": "svc", "port": 80, "filters": [
                {"type": "ResponseHeaderModifier"}
            ]}]},
            {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                "set": [{"name": "x-valid", "value": "yes"}]
            }}]}
        ]));
        route.spec.parent_refs = Some(serde_json::from_value(json!([{"name": "gw"}])).unwrap());
        let handler = GrpcRouteHandler::default();
        let ctx = make_ctx();
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|c| c.type_ == "PartiallyInvalid"
            && c.status == "True"
            && c.message.starts_with("Dropped Rule")
            && c.message.contains("1 of 3 rules are active")));
        assert!(!conditions.iter().any(|c| c.message.contains("secret-marker")));

        route.spec.rules.as_mut().unwrap().truncate(2);
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|c| c.type_ == "Accepted"
            && c.status == "False"
            && c.message.contains("rules[0]")
            && c.message.contains("rules[1]")));
        assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

        route.spec.rules.as_mut().unwrap()[0].filters = None;
        route.spec.rules.as_mut().unwrap()[1].backend_refs.as_mut().unwrap()[0].filters = None;
        handler.update_status(&mut route, &ctx, &[]);
        assert!(route.spec.invalid_rule_indices.is_none());
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));
        assert!(!conditions.iter().any(|c| c.message.contains("rules[0]")));
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
                    { "type": "ExtensionRef", "extensionRef": { "group": "edgion.io", "kind": "EdgionPlugins", "name": "b" } }
                ]
            }
        ]));
        assert!(super::find_invalid_rule_indices(&route, "default").is_empty());
    }

    #[test]
    fn extension_ref_pairing_errors_drop_only_the_affected_grpc_rules() {
        let mut route = route_with_rules(json!([
            {"filters": [{"type": "ExtensionRef"}]},
            {"filters": [{"type": "RequestHeaderModifier", "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "unexpected"
            }}]},
            {"backendRefs": [{"name": "svc", "port": 80, "filters": [
                {"type": "ExtensionRef"}
            ]}]},
            {"filters": [{"type": "ExtensionRef", "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "valid"
            }}]}
        ]));
        route.spec.parent_refs = Some(serde_json::from_value(json!([{"name": "gw"}])).unwrap());

        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0, 1, 2]);
        GrpcRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1, 2][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == "PartiallyInvalid"
                && condition.status == "True"
                && condition.message.starts_with("Dropped Rule")
        }));

        route.spec.rules.as_mut().unwrap().truncate(3);
        GrpcRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == "Accepted" && condition.status == "False" && condition.message.contains("rules[0]")
        }));
        assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
    }

    #[test]
    fn extension_ref_empty_group_is_not_edgion_io() {
        let route = route_with_rules(json!([{
            "filters": [{"type": "ExtensionRef", "extensionRef": {
                "kind": "EdgionPlugins", "name": "plugins"
            }}]
        }]));
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
    }

    #[test]
    fn invalid_session_persistence_marks_grpc_rule_invalid() {
        let route: GRPCRoute = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": { "name": "r", "namespace": "default" },
            "spec": {
                "rules": [{
                    "sessionPersistence": { "absoluteTimeout": "1d" }
                }]
            }
        }))
        .expect("route should deserialize");
        assert_eq!(super::find_invalid_rule_indices(&route, "default"), vec![0]);
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

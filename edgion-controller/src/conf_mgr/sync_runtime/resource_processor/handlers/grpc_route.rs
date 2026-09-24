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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route_test.rs"]
mod tests;

//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::{is_cross_ns_ref_allowed, validate_http_route_if_enabled};
use super::route_delegation::{flatten_route, populate_terminal_rule_identities, DelegationLookup};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_policies_for_ai_targets,
    requeue_backend_policies_for_route, requeue_backend_policies_for_services, requeue_parent_gateways,
    update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, set_parent_conditions_full, update_condition,
    AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::HTTPRoute;
use edgion_resources::resources::common::{ParentReference, RefDenied};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins};
use edgion_resources::resources::http_route::{
    DelegationIssue, HTTPBackendRefKind, HTTPRouteFilterType, HTTPRouteRule, HTTPRouteStatus, RouteParentStatus,
    GATEWAY_API_GROUP,
};
use edgion_resources::ResourceKind;

/// True for parentRef entries that attach the route to a Gateway listener
/// (the Gateway API default when group/kind are absent). Delegation
/// parentRefs (`kind: HTTPRoute`, reverse-lock declarations) return false.
fn is_gateway_parent_ref(pr: &ParentReference) -> bool {
    pr.group.as_deref().unwrap_or(GATEWAY_API_GROUP) == GATEWAY_API_GROUP
        && pr.kind.as_deref().unwrap_or("Gateway") == "Gateway"
}

/// True for parentRef entries that reverse-lock delegation to specific parent
/// HTTPRoutes.
fn is_delegation_parent_ref(pr: &ParentReference) -> bool {
    pr.group.as_deref().unwrap_or(GATEWAY_API_GROUP) == GATEWAY_API_GROUP && pr.kind.as_deref() == Some("HTTPRoute")
}

/// `DelegationLookup` backed by the live controller stores.
struct RuntimeDelegationLookup<'a> {
    runtime: &'a ControllerRuntime,
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
            "HTTPRoute",
            to_ns,
            group.as_ref(),
            kind.as_ref(),
            name,
            self.runtime,
        )
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
    /// `extra_tuples` carries the flattened delegation backendRefs (with
    /// explicit namespaces) so Service changes under delegated children
    /// requeue this parent route directly. Registration replaces the
    /// resource's whole ref set, so own-rule and flattened refs must land in
    /// this single call.
    fn register_service_refs(
        route: &HTTPRoute,
        extra_tuples: &[(Option<String>, Option<String>, Option<String>, String)],
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
            ResourceKind::HTTPRoute,
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
    /// requeue this parent route. Registration replaces the resource's whole
    /// ref set, so own-rule and flattened targets must land in this single call.
    fn record_cross_ns_refs(route: &HTTPRoute, extra_namespaces: &HashSet<String>, ctx: &HandlerContext) {
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

    /// Register rule-level EdgionPlugins ExtensionRefs for plugin-change requeue.
    ///
    /// BackendRef filters deliberately do not participate: AiProxy is a
    /// route-rule policy, and the AI routing contract requires exactly one
    /// rule-level reference. Replacing the full ref set on every reconcile
    /// also handles delete-to-create coalescing.
    fn register_plugin_refs(route: &HTTPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().plugin_ref_mgr;
        manager.clear_resource_refs(&resource_ref);

        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        for rule in route.effective_rules().into_iter().flatten() {
            for filter in rule.filters.iter().flatten() {
                if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
                    continue;
                }
                let Some(extension_ref) = &filter.extension_ref else {
                    continue;
                };
                if (extension_ref.group.is_empty() || extension_ref.group == "edgion.io")
                    && extension_ref.kind == "EdgionPlugins"
                {
                    let ref_namespace = extension_ref.resolved_namespace.as_deref().unwrap_or(namespace);
                    manager.add_ref(format!("{ref_namespace}/{}", extension_ref.name), resource_ref.clone());
                }
            }
        }
    }

    /// Replace this route's EdgionAIService dependency set and return the
    /// old/new union plus the removed subset. The union is needed because an
    /// EBTP attached to either side may change the route's Gateway ancestors.
    fn register_ai_service_refs(
        route: &HTTPRoute,
        flattened_tuples: &[(Option<String>, Option<String>, Option<String>, String)],
        ctx: &HandlerContext,
    ) -> AiRefDelta {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().ai_service_ref_mgr;
        let old: HashSet<String> = manager.clear_resource_refs(&resource_ref).into_iter().collect();
        let canonical_key = |group: Option<&str>, kind: Option<&str>, namespace: Option<&str>, name: &str| {
            if group == Some("edgion.io") && kind == Some("EdgionAIService") {
                Some(
                    PolicyTargetKey {
                        target_kind: PolicyTargetKind::EdgionAIService,
                        namespace: namespace.unwrap_or(route_ns).to_string(),
                        name: name.to_string(),
                    }
                    .to_string(),
                )
            } else {
                None
            }
        };

        let mut effective = HashSet::new();
        for rule in route.effective_rules().into_iter().flatten() {
            for backend_ref in rule.backend_refs.iter().flatten() {
                if let Some(key) = canonical_key(
                    backend_ref.group.as_deref(),
                    backend_ref.kind.as_deref(),
                    backend_ref.namespace.as_deref(),
                    &backend_ref.name,
                ) {
                    effective.insert(key);
                }
            }
        }
        let new: HashSet<String> = if route.spec.resolved_rules.is_some() {
            // FlattenOutcome preserves each declaring child namespace. Keep
            // only tuples whose rule survived Controller-side AI validation.
            flattened_tuples
                .iter()
                .filter_map(|(group, kind, namespace, name)| {
                    canonical_key(group.as_deref(), kind.as_deref(), namespace.as_deref(), name)
                })
                .filter(|key| effective.contains(key))
                .collect()
        } else {
            effective
        };
        for key in &new {
            manager.add_ref(key.clone(), resource_ref.clone());
        }
        let mut affected: Vec<String> = old.union(&new).cloned().collect();
        affected.sort_unstable();
        let mut dropped: Vec<String> = old.difference(&new).cloned().collect();
        dropped.sort_unstable();
        AiRefDelta { affected, dropped }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AiRefDelta {
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
    fn validate(&self, route: &HTTPRoute, _ctx: &HandlerContext) -> Vec<String> {
        let errors = validate_http_route_if_enabled(route);
        // Note: per-rule validation (filter conflicts, malformed CORS) is handled by
        // find_invalid_rule_details() in update_status(), which marks individual rules
        // as invalid via PartiallyInvalid condition rather than rejecting the entire route.
        errors
    }

    async fn parse(&self, mut route: HTTPRoute, _ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let route_ns = route_ns.as_str();

        // Terminal identity is controller-owned transport metadata. Stamp the
        // root rules before flattening so non-delegating siblings inherit it;
        // delegated child terminals are stamped by the flattener itself.
        populate_terminal_rule_identities(&mut route);

        // Mark denied cross-namespace references on the operator-authored rules.
        // MUST run BEFORE the delegation flatten: flatten clones non-delegating
        // sibling rules verbatim into resolved_rules, and the gateway compiles
        // resolved_rules INSTEAD of spec.rules — marking afterwards would leave
        // the compiled copies with ref_denied=None (ReferenceGrant bypass).
        if let Some(rules) = &mut route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &mut rule.backend_refs {
                    for backend_ref in backend_refs {
                        // Delegation refs (kind: HTTPRoute) are not Service
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
                                    "HTTPRoute",
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

        // HTTPRoute delegation: expand delegation trees into resolved_rules.
        // Runs early so the flattened Service/cross-ns refs land in the single
        // registration pass below. resolved_rules/delegation_issues are
        // controller-derived: clear any user-provided or stale values.
        route.spec.resolved_rules = None;
        route.spec.delegation_issues = Vec::new();
        let mut delegation_outcome = {
            let adapter = RuntimeDelegationLookup {
                runtime: _ctx.runtime(),
            };
            flatten_route(&route, &adapter)
        };
        // Dependency registration below must inspect the rules that the
        // Gateway will actually compile, including delegated child filters.
        route.spec.resolved_rules = delegation_outcome.resolved_rules.clone();
        if let Some(rules) = &mut route.spec.resolved_rules {
            let mut resolved_index = 0usize;
            rules.retain(|rule| {
                let reason = ai_rule_invalid_reason(rule, route_ns, |namespace, name| {
                    enabled_ai_proxy_count(_ctx.runtime(), namespace, name)
                });
                let keep = reason.is_none();
                if let Some(reason) = reason {
                    delegation_outcome.issues.push(DelegationIssue::RuleDropped {
                        message: format!("resolvedRules[{resolved_index}]: {reason}"),
                    });
                }
                resolved_index += 1;
                keep
            });
        }

        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route, &delegation_outcome.cross_ns_target_namespaces, _ctx);

        // Track rule-level EdgionPlugins references so plugin changes recompile
        // every affected HTTPRoute through its normal workqueue.
        Self::register_plugin_refs(&route, _ctx);

        let ai_ref_delta = Self::register_ai_service_refs(&route, &delegation_outcome.ai_service_backend_tuples, _ctx);
        requeue_backend_policies_for_ai_targets(&ai_ref_delta.affected, _ctx).await;
        tracing::debug!(
            route = %format!("{route_ns}/{}", route.metadata.name.as_deref().unwrap_or("")),
            dropped_ai_services = ?ai_ref_delta.dropped,
            "Replaced HTTPRoute EdgionAIService references"
        );

        // Register Service backend references for cross-resource requeue, then
        // requeue BTPs bound to any Service dropped from backendRefs (the apply
        // path otherwise only requeues BTPs for the route's current backends).
        let dropped_service_keys =
            Self::register_service_refs(&route, &delegation_outcome.service_backend_tuples, _ctx);
        requeue_backend_policies_for_services(&dropped_service_keys, _ctx).await;

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
        requeue_backend_policies_for_route(ResourceKind::HTTPRoute, route_ns, route_name, ctx).await;

        // This route may be someone's delegation child: requeue the parents so
        // they re-flatten with the new child spec.
        let route_key = format!("{route_ns}/{route_name}");
        for parent_key in ctx.runtime().route_delegation_index.parents_of(&route_key, route_ns) {
            ctx.requeue("HTTPRoute", parent_key).await;
        }
    }

    async fn on_delete(&self, route: &HTTPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_resource_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        requeue_backend_policies_for_route(ResourceKind::HTTPRoute, route_ns, route_name, ctx).await;
        let removed_ai_targets = ctx.runtime().ai_service_ref_mgr.clear_resource_refs(&resource_ref);
        requeue_backend_policies_for_ai_targets(&removed_ai_targets, ctx).await;
        super::route_utils::clear_service_backend_refs(ResourceKind::HTTPRoute, route_ns, route_name, ctx.runtime());

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
            let adapter = RuntimeDelegationLookup { runtime: ctx.runtime() };
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

        let tracker_changed =
            remove_from_attached_route_tracker(ResourceKind::HTTPRoute, route_ns, route_name, ctx.runtime());
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_status(&self, route: &mut HTTPRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        for (index, rule) in route.spec.rules.iter().flatten().enumerate() {
            if let Some(message) = ai_rule_invalid_reason(rule, route_ns, |namespace, name| {
                enabled_ai_proxy_count(_ctx.runtime(), namespace, name)
            }) {
                validation_accepted_errors.push(AcceptedError::UnsupportedValue {
                    message: format!("rules[{index}]: {message}"),
                });
            }
        }

        let mut resolved_refs_errors = validate_http_backend_refs(route, route_ns, _ctx.runtime());
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
        let invalid_details = find_controller_invalid_rule_details(route, route_ns, _ctx.runtime());
        let invalid_indices: Vec<usize> = invalid_details.iter().map(|(i, _)| *i).collect();
        let total_rules = route.spec.rules.as_ref().map_or(0, |r| r.len());
        let valid_rules = total_rules - invalid_indices.len();

        route.spec.invalid_rule_indices = if invalid_indices.is_empty() {
            None
        } else {
            Some(invalid_indices.clone())
        };

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
                accepted_errors.extend(validation_accepted_errors.clone());

                // Note: per-rule problems (rewrite/redirect conflict, malformed CORS,
                // unreachable backendRefs) do NOT flip Accepted to False — they are
                // surfaced via PartiallyInvalid (or ResolvedRefs for backend issues).
                // Per Gateway API spec, Accepted reflects whether the Gateway accepts
                // the route at all; flipping it to False would drop the entire route
                // from the data-plane route table.

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
                        reasons.extend(invalid_details.iter().map(|(i, r)| format!("rules[{i}]: {r}")));
                    }
                    reasons.extend(delegation_dropped.iter().cloned());
                    let mut message = reasons.join("; ");
                    if own_invalid {
                        message = format!("{message} ({valid_rules} of {total_rules} rules active)");
                    }
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
                let condition = if delegating_parents.contains(&parent_key) {
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
            ResourceKind::HTTPRoute,
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
/// Delegation backendRefs (`kind: HTTPRoute`) are excluded — they are not
/// Service backends; their resolution problems are reported through
/// `DelegationIssue` (MissingChild → ResolvedRefs, drops → PartiallyInvalid).
fn validate_http_backend_refs(
    route: &HTTPRoute,
    route_ns: &str,
    runtime: &ControllerRuntime,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for rule in route.effective_rules().into_iter().flatten() {
        for backend_ref in rule.backend_refs.iter().flatten() {
            let namespace = backend_ref.namespace.as_deref().unwrap_or(route_ns);
            match backend_ref.backend_kind() {
                HTTPBackendRefKind::Service => {
                    if !super::route_utils::service_exists(namespace, &backend_ref.name, runtime) {
                        errors.push(ResolvedRefsError::BackendNotFound {
                            namespace: namespace.to_string(),
                            name: backend_ref.name.clone(),
                        });
                    }
                }
                HTTPBackendRefKind::EdgionAIService => {
                    if backend_ref.port.is_some() {
                        errors.push(ResolvedRefsError::InvalidKind {
                            kind: "edgion.io/EdgionAIService with port".to_string(),
                            name: backend_ref.name.clone(),
                        });
                        continue;
                    }
                    let exists = runtime
                        .processor_registry
                        .get("EdgionAIService")
                        .is_none_or(|processor| processor.contains_key(&format!("{namespace}/{}", backend_ref.name)));
                    if !exists {
                        errors.push(ResolvedRefsError::EdgionAIServiceNotFound {
                            namespace: namespace.to_string(),
                            name: backend_ref.name.clone(),
                        });
                    }
                }
                HTTPBackendRefKind::RouteDelegation => {}
                HTTPBackendRefKind::Unsupported => errors.push(ResolvedRefsError::InvalidKind {
                    kind: format!(
                        "{}/{}",
                        backend_ref.group.as_deref().unwrap_or(""),
                        backend_ref.kind.as_deref().unwrap_or("Service")
                    ),
                    name: backend_ref.name.clone(),
                }),
            }
        }
    }
    errors
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

/// Check if a filter list contains both URLRewrite and RequestRedirect.
///
/// Per Gateway API spec, these two filters are mutually exclusive within a single rule.
fn has_rewrite_redirect_conflict(
    filters: Option<&Vec<edgion_resources::resources::http_route::HTTPRouteFilter>>,
) -> bool {
    let Some(filters) = filters else { return false };
    let has_rewrite = filters.iter().any(|f| f.filter_type == HTTPRouteFilterType::URLRewrite);
    let has_redirect = filters
        .iter()
        .any(|f| f.filter_type == HTTPRouteFilterType::RequestRedirect);
    has_rewrite && has_redirect
}

/// Return `Some(reason)` if any `type: CORS` filter on the rule (rule-level or
/// per-backendRef) is malformed: either `cors:` config is absent, or
/// `HTTPCORSFilter::validate()` rejects the origin format. Reason text is
/// operator-readable and lands in the `PartiallyInvalid` condition message.
///
/// Rules flagged here go into `invalid_rule_indices` → `PartiallyInvalid=True`
/// while the route stays `Accepted=True`. This matches the rewrite/redirect
/// filter-conflict precedent and avoids dropping the entire route from the
/// data-plane route table on misconfiguration.
fn invalid_cors_reason(rule: &edgion_resources::resources::http_route::HTTPRouteRule) -> Option<String> {
    let filter_reason = |f: &edgion_resources::resources::http_route::HTTPRouteFilter| -> Option<String> {
        if f.filter_type != HTTPRouteFilterType::CORS {
            return None;
        }
        match f.cors.as_ref() {
            None => Some("CORS filter missing 'cors' configuration block".to_string()),
            Some(cfg) => cfg.validate().err().map(|errs| format!("CORS {}", errs.join("; "))),
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

/// Returns a reason string if a rule has an `ExtensionRef` filter whose `kind`
/// is not a recognized Edgion extension kind.
///
/// `ExtensionRef` has exactly one recognized use: `EdgionPlugins` (group empty
/// or `edgion.io`) — a plugin reference. The former route-level
/// extensionRef-based load-balancing-policy mechanism has been removed; those
/// kinds are now flagged like any other unsupported kind. Load-balancing
/// policy is configured via `EdgionBackendTrafficPolicy`.
///
/// Anything else is a static config error knowable without the runtime store;
/// flagging it here excludes the rule from the data-plane route table instead
/// of leaving it to a silent runtime `GoodNext`.
fn invalid_extension_ref_reason(rule: &edgion_resources::resources::http_route::HTTPRouteRule) -> Option<String> {
    let filter_reason = |f: &edgion_resources::resources::http_route::HTTPRouteFilter| -> Option<String> {
        if f.filter_type != HTTPRouteFilterType::ExtensionRef {
            return None;
        }
        let ext = f.extension_ref.as_ref()?;
        let group_ok = ext.group.is_empty() || ext.group == "edgion.io";
        if group_ok && ext.kind == "EdgionPlugins" {
            return None;
        }
        Some(format!(
            "ExtensionRef kind '{}' (group '{}') is not supported; only EdgionPlugins is",
            ext.kind, ext.group
        ))
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

fn enabled_ai_proxy_count(runtime: &ControllerRuntime, namespace: &str, name: &str) -> Option<usize> {
    let processor = runtime.processor_registry.get("EdgionPlugins")?;
    let key = format!("{namespace}/{name}");
    if !processor.contains_key(&key) {
        return None;
    }
    let plugins = processor.get_raw_any(&key)?.downcast::<EdgionPlugins>().ok()?;
    Some(
        plugins
            .spec
            .request_plugins
            .iter()
            .flatten()
            .filter(|entry| entry.enable && matches!(entry.plugin, EdgionPlugin::AiProxy(_)))
            .count(),
    )
}

/// Enforce the Controller-owned AI routing contract for one effective rule.
/// The lookup returns the number of enabled request-stage AiProxy entries in a
/// referenced EdgionPlugins object, or `None` when that object is unavailable.
fn ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    mut lookup: impl FnMut(&str, &str) -> Option<usize>,
) -> Option<String> {
    let backend_refs: Vec<_> = rule.backend_refs.iter().flatten().collect();
    let ai_backends: Vec<_> = backend_refs
        .iter()
        .copied()
        .filter(|backend_ref| backend_ref.backend_kind() == HTTPBackendRefKind::EdgionAIService)
        .collect();

    if !ai_backends.is_empty() {
        if rule
            .retry
            .as_ref()
            .and_then(|retry| retry.attempts)
            .is_some_and(|attempts| attempts > 7)
        {
            return Some("AI routing retry.attempts must not exceed 7".to_string());
        }
        if ai_backends.len() != backend_refs.len() {
            return Some("EdgionAIService backends must not be mixed with other backend kinds".to_string());
        }
        if ai_backends.iter().any(|backend_ref| backend_ref.port.is_some()) {
            return Some("EdgionAIService backendRefs must not set port".to_string());
        }
        if let Some(filter) = ai_backends
            .iter()
            .flat_map(|backend_ref| backend_ref.filters.iter().flatten())
            .find(|filter| {
                !matches!(
                    filter.filter_type,
                    HTTPRouteFilterType::RequestHeaderModifier | HTTPRouteFilterType::ResponseHeaderModifier
                )
            })
        {
            return Some(format!(
                "EdgionAIService backendRefs do not support {:?} filters",
                filter.filter_type
            ));
        }
    }

    let mut ai_proxy_count = 0usize;
    let mut missing_plugin = None;
    for filter in rule.filters.iter().flatten() {
        if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
            continue;
        }
        let Some(extension_ref) = &filter.extension_ref else {
            continue;
        };
        if !(extension_ref.group.is_empty() || extension_ref.group == "edgion.io")
            || extension_ref.kind != "EdgionPlugins"
        {
            continue;
        }
        let namespace = extension_ref.resolved_namespace.as_deref().unwrap_or(route_namespace);
        match lookup(namespace, &extension_ref.name) {
            Some(count) => ai_proxy_count += count,
            None => missing_plugin = Some(format!("{namespace}/{}", extension_ref.name)),
        }
    }

    if ai_backends.is_empty() {
        return (ai_proxy_count > 0)
            .then(|| "AiProxy requires a rule whose forwarding backends are all EdgionAIService".to_string());
    }
    if let Some(key) = missing_plugin {
        return Some(format!(
            "AI routing requires exactly one enabled AiProxy, but EdgionPlugins '{key}' was not found"
        ));
    }
    (ai_proxy_count != 1).then(|| format!("AI routing requires exactly one enabled AiProxy (found {ai_proxy_count})"))
}

fn find_controller_invalid_rule_details(
    route: &HTTPRoute,
    route_ns: &str,
    runtime: &ControllerRuntime,
) -> Vec<(usize, String)> {
    let mut details = find_invalid_rule_details(route, route_ns);
    let already_invalid: HashSet<usize> = details.iter().map(|(index, _)| *index).collect();
    for (index, rule) in route.spec.rules.iter().flatten().enumerate() {
        if already_invalid.contains(&index) {
            continue;
        }
        if let Some(reason) = ai_rule_invalid_reason(rule, route_ns, |namespace, name| {
            enabled_ai_proxy_count(runtime, namespace, name)
        }) {
            details.push((index, reason));
        }
    }
    details.sort_by_key(|(index, _)| *index);
    details
}

/// Find rules that are invalid, returning `(index, reason)` for each.
///
/// A rule is invalid if:
/// - It has conflicting URLRewrite + RequestRedirect filters (rule-level, per-backendRef, or cross-level)
/// - It has a malformed CORS filter (missing `cors:` block, or origin fails Gateway API v1.5 validation)
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
        if has_rewrite_redirect_conflict(rule.filters.as_ref()) {
            details.push((idx, "filter conflict: URLRewrite + RequestRedirect".to_string()));
            continue;
        }
        if let Some(reason) = invalid_cors_reason(rule) {
            details.push((idx, reason));
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
        if let Some(backend_refs) = &rule.backend_refs {
            let backend_conflict = backend_refs.iter().any(|br| {
                if has_rewrite_redirect_conflict(br.filters.as_ref()) {
                    return true;
                }
                // Cross-level: rule filters + backend filters combined.
                if let (Some(rule_filters), Some(br_filters)) = (rule.filters.as_ref(), br.filters.as_ref()) {
                    let combined_has_rewrite = rule_filters
                        .iter()
                        .chain(br_filters.iter())
                        .any(|f| f.filter_type == HTTPRouteFilterType::URLRewrite);
                    let combined_has_redirect = rule_filters
                        .iter()
                        .chain(br_filters.iter())
                        .any(|f| f.filter_type == HTTPRouteFilterType::RequestRedirect);
                    if combined_has_rewrite && combined_has_redirect {
                        return true;
                    }
                }
                false
            });
            if backend_conflict {
                details.push((
                    idx,
                    "filter conflict: URLRewrite + RequestRedirect on backendRef".to_string(),
                ));
                continue;
            }
        }
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
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

    fn route_with_one_cors_rule(origins: &[&str]) -> HTTPRoute {
        let origins_yaml = origins
            .iter()
            .map(|o| format!("            - \"{o}\""))
            .collect::<Vec<_>>()
            .join("\n");
        let yaml = format!(
            r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: CORS
          cors:
            allowOrigins:
{origins_yaml}
"#
        );
        serde_yaml::from_str(&yaml).expect("HTTPRoute yaml must parse")
    }

    fn indices_only(details: Vec<(usize, String)>) -> Vec<usize> {
        details.into_iter().map(|(i, _)| i).collect()
    }

    fn ai_rule(backends: serde_json::Value, filters: serde_json::Value) -> HTTPRouteRule {
        serde_json::from_value(serde_json::json!({
            "backendRefs": backends,
            "filters": filters,
        }))
        .expect("AI rule must parse")
    }

    #[test]
    fn ai_rule_requires_homogeneous_backends_and_exactly_one_ai_proxy() {
        let extension_ref = serde_json::json!([{
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io",
                "kind": "EdgionPlugins",
                "name": "ai",
                "resolvedNamespace": "child"
            }
        }]);
        let pure_ai = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionAIService", "name": "models"
            }]),
            extension_ref.clone(),
        );
        assert_eq!(
            ai_rule_invalid_reason(&pure_ai, "parent", |namespace, name| {
                assert_eq!((namespace, name), ("child", "ai"));
                Some(1)
            }),
            None
        );

        let missing = ai_rule_invalid_reason(&pure_ai, "parent", |_, _| Some(0)).unwrap();
        assert!(missing.contains("exactly one enabled AiProxy"), "{missing}");

        let mixed = ai_rule(
            serde_json::json!([
                {"group": "edgion.io", "kind": "EdgionAIService", "name": "models"},
                {"name": "ordinary", "port": 80}
            ]),
            extension_ref,
        );
        let reason = ai_rule_invalid_reason(&mixed, "parent", |_, _| Some(1)).unwrap();
        assert!(reason.contains("must not be mixed"), "{reason}");
    }

    #[test]
    fn ai_backend_filters_allow_only_header_modifiers() {
        let extension_ref = serde_json::json!([{
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
            }
        }]);
        let allowed = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionAIService", "name": "models",
                "filters": [
                    {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-tenant", "value": "a"}]}},
                    {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"set": [{"name": "x-ai", "value": "yes"}]}}
                ]
            }]),
            extension_ref.clone(),
        );
        assert!(ai_rule_invalid_reason(&allowed, "default", |_, _| Some(1)).is_none());

        let rejected = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionAIService", "name": "models",
                "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {"type": "ReplaceFullPath", "replaceFullPath": "/v2"}}}]
            }]),
            extension_ref,
        );
        let reason = ai_rule_invalid_reason(&rejected, "default", |_, _| Some(1)).unwrap();
        assert!(reason.contains("URLRewrite"), "{reason}");
    }

    #[test]
    fn ai_rule_rejects_more_than_seven_explicit_retries() {
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
            "backendRefs": [{
                "group": "edgion.io", "kind": "EdgionAIService", "name": "models"
            }],
            "filters": [{
                "type": "ExtensionRef",
                "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
                }
            }],
            "retry": {"attempts": 8}
        }))
        .expect("AI rule must parse");

        let reason = ai_rule_invalid_reason(&rule, "default", |_, _| Some(1)).unwrap();
        assert!(reason.contains("must not exceed 7"), "{reason}");
    }

    #[test]
    fn ai_and_plugin_reverse_refs_replace_old_effective_dependencies() {
        let ctx = make_ctx();
        let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "parent", "namespace": "infra"},
            "spec": {
                "resolvedRules": [{
                    "backendRefs": [{
                        "group": "edgion.io", "kind": "EdgionAIService",
                        "name": "models-a", "namespace": "child"
                    }],
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": {
                            "group": "edgion.io", "kind": "EdgionPlugins",
                            "name": "ai-plugins", "resolvedNamespace": "child"
                        }
                    }]
                }]
            }
        }))
        .unwrap();
        let route_ref = HttpRouteHandler::create_resource_ref(&route);

        let first_tuples = vec![(
            Some("edgion.io".to_string()),
            Some("EdgionAIService".to_string()),
            Some("child".to_string()),
            "models-a".to_string(),
        )];
        let first = HttpRouteHandler::register_ai_service_refs(&route, &first_tuples, &ctx);
        assert_eq!(
            first.affected,
            vec!["edgion.io/EdgionAIService/child/models-a".to_string()]
        );
        HttpRouteHandler::register_plugin_refs(&route, &ctx);
        assert_eq!(
            ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key()),
            vec!["child/ai-plugins".to_string()]
        );

        let resolved = route.spec.resolved_rules.as_mut().unwrap();
        resolved[0].backend_refs.as_mut().unwrap()[0].name = "models-b".to_string();
        resolved[0].filters = None;
        let second_tuples = vec![(
            Some("edgion.io".to_string()),
            Some("EdgionAIService".to_string()),
            Some("child".to_string()),
            "models-b".to_string(),
        )];
        let second = HttpRouteHandler::register_ai_service_refs(&route, &second_tuples, &ctx);
        assert_eq!(
            second.dropped,
            vec!["edgion.io/EdgionAIService/child/models-a".to_string()]
        );
        assert_eq!(second.affected.len(), 2);
        HttpRouteHandler::register_plugin_refs(&route, &ctx);
        assert!(ctx
            .runtime()
            .plugin_ref_mgr
            .get_dependencies(&route_ref.key())
            .is_empty());
    }

    #[tokio::test]
    async fn parse_marks_ref_denied_before_flatten_copies_siblings() {
        // Regression (P0): flatten copies non-delegating sibling rules into
        // resolved_rules, which the gateway compiles INSTEAD of spec.rules.
        // The ReferenceGrant marking must therefore run BEFORE flatten so the
        // copies carry ref_denied — otherwise an ungranted cross-ns Service
        // ref on a delegating parent would be silently routable.
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "parent", "namespace": "infra"},
            "spec": {
                "rules": [
                    {
                        "matches": [{"path": {"type": "PathPrefix", "value": "/plain"}}],
                        "backendRefs": [{"name": "secret-svc", "namespace": "secret-ns", "port": 80}]
                    },
                    {
                        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                        "backendRefs": [{
                            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                            "name": "*", "namespace": "billing"
                        }]
                    }
                ]
            }
        }))
        .unwrap();

        let handler = HttpRouteHandler::default();
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
                    .any(|m| m.path.as_ref().and_then(|p| p.value.as_deref()) == Some("/plain"))
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
        // Regression: a cross-namespace delegation backendRef (kind: HTTPRoute)
        // must not go through the ReferenceGrant check — delegation is
        // authorized by the parent opt-in, and a spurious ref_denied surfaced
        // as ResolvedRefs=False/RefNotPermitted on the parent status.
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "parent", "namespace": "infra"},
            "spec": {
                "rules": [{
                    "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                    "backendRefs": [{
                        "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                        "name": "*", "namespace": "billing"
                    }]
                }]
            }
        }))
        .unwrap();

        let handler = HttpRouteHandler::default();
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
        // And HTTP-specific validation excludes it (no InvalidKind on status path).
        assert!(validate_http_backend_refs(&parsed, "infra", ctx.runtime()).is_empty());
    }

    #[test]
    fn invalid_cors_origin_flags_rule_with_reason() {
        let route = route_with_one_cors_rule(&["evil.com"]);
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        let (_, reason) = &details[0];
        assert!(
            reason.contains("CORS") && reason.contains("evil.com"),
            "reason: {reason}"
        );
    }

    #[test]
    fn cors_origin_with_path_flags_rule_with_reason() {
        let route = route_with_one_cors_rule(&["https://example.com/path"]);
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        let (_, reason) = &details[0];
        assert!(
            reason.contains("CORS") && reason.contains("https://example.com/path"),
            "reason: {reason}"
        );
    }

    #[test]
    fn cors_filter_without_cors_block_flags_rule_with_reason() {
        // `type: CORS` with no `cors:` config — gateway-side silently drops the
        // filter and CORS protection is off. Must be flagged.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: CORS
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        assert!(
            details[0].1.contains("missing 'cors' configuration block"),
            "reason: {}",
            details[0].1
        );
    }

    #[test]
    fn valid_cors_does_not_flag_rule() {
        let route = route_with_one_cors_rule(&["https://good.example.com", "https://*.foo.com:8443"]);
        assert!(find_invalid_rule_details(&route, "default").is_empty());
    }

    #[test]
    fn no_cors_filter_does_not_flag_rule() {
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-test
                value: "1"
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        assert!(find_invalid_rule_details(&route, "default").is_empty());
    }

    #[test]
    fn invalid_cors_on_backend_ref_flags_rule_with_backendref_prefix() {
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - backendRefs:
        - name: svc
          port: 80
          filters:
            - type: CORS
              cors:
                allowOrigins:
                  - "no-scheme.example.com"
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        assert!(
            details[0].1.starts_with("backendRefs[0] CORS"),
            "expected backendRef-prefixed reason; got: {}",
            details[0].1
        );
    }

    #[test]
    fn invalid_cors_only_flags_the_offending_rule() {
        // Two rules: rule[0] has bad CORS, rule[1] has valid CORS. Only rule[0]
        // should be flagged so the route stays partially active.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: CORS
          cors:
            allowOrigins: ["bad.example.com"]
    - filters:
        - type: CORS
          cors:
            allowOrigins: ["https://good.example.com"]
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(indices_only(find_invalid_rule_details(&route, "default")), vec![0]);
    }

    #[test]
    fn invalid_session_persistence_flags_only_offending_rule() {
        // rule[0]: Permanent lifetime without absoluteTimeout -> invalid.
        // rule[1]: valid session persistence. Only rule[0] is flagged.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - sessionPersistence:
        cookieConfig:
          lifetimeType: Permanent
    - sessionPersistence:
        absoluteTimeout: 1h
        cookieConfig:
          lifetimeType: Permanent
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        assert!(
            details[0].1.contains("absoluteTimeout"),
            "expected absoluteTimeout reason; got: {}",
            details[0].1
        );
    }

    #[test]
    fn invalid_session_persistence_bad_duration_flags_rule() {
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - sessionPersistence:
        absoluteTimeout: 1d
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        assert!(details[0].1.contains("sessionPersistence"), "got: {}", details[0].1);
    }

    #[test]
    fn invalid_cors_results_in_partially_invalid_not_accepted_false() {
        // End-to-end: handler.update_status on a route with bad CORS must produce
        // PartiallyInvalid=True, Accepted=True. This is the contract that protects
        // against the upgrade-time outage risk: bad CORS must not nuke the whole
        // route from the gateway route table.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
  generation: 7
spec:
  parentRefs:
    - name: gw
      namespace: default
  rules:
    - filters:
        - type: CORS
          cors:
            allowOrigins: ["bad.example.com"]
    - filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-test
                value: "1"
"#;
        let handler = HttpRouteHandler::default();
        let mut route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        handler.update_status(&mut route, &make_ctx(), &[]);

        let parents = &route.status.as_ref().expect("status").parents;
        assert_eq!(parents.len(), 1);
        let conds = &parents[0].conditions;
        let accepted = conds.iter().find(|c| c.type_ == "Accepted").expect("Accepted set");
        let partial = conds
            .iter()
            .find(|c| c.type_ == "PartiallyInvalid")
            .expect("PartiallyInvalid set");
        // Note: Accepted may be False here because the parent Gateway 'gw' doesn't
        // exist in the test runtime (NoMatchingParent). What matters is that the
        // bad-CORS rule did NOT push the route into Accepted=False via reason=Invalid —
        // it should be marked as a rule-level problem via PartiallyInvalid.
        assert_ne!(
            accepted.reason, "Invalid",
            "bad CORS must not produce Accepted=False with reason=Invalid (would drop whole route); got: {accepted:?}"
        );
        assert_eq!(
            partial.status, "True",
            "PartiallyInvalid must be True; got: {partial:?}"
        );
        assert_eq!(partial.reason, "RuleInvalid");
        assert_eq!(
            route.spec.invalid_rule_indices.as_deref(),
            Some(&[0usize][..]),
            "invalid_rule_indices must contain the bad-CORS rule index"
        );
        // Operator-debuggability: the PartiallyInvalid message must carry both
        // the rule index and the CORS reason so operators don't have to grep
        // gateway logs to find out which origin was rejected.
        assert!(
            partial.message.contains("rules[0]") && partial.message.contains("CORS"),
            "PartiallyInvalid message must include rule index + CORS reason; got: {}",
            partial.message
        );
        assert!(
            partial.message.contains("1 of 2 rules active"),
            "PartiallyInvalid message must include valid/total rule count; got: {}",
            partial.message
        );
    }

    #[test]
    fn extension_ref_unsupported_kind_marks_rule_invalid() {
        // A rule with an ExtensionRef filter pointing at a non-EdgionPlugins
        // kind must be flagged invalid.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: NotAPlugin
            name: x
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(
            indices_only(details.clone()),
            vec![0],
            "rule 0 must be flagged: {details:?}"
        );
        assert!(
            details[0].1.contains("ExtensionRef") && details[0].1.contains("NotAPlugin"),
            "reason must name the unsupported kind: {}",
            details[0].1
        );
    }

    #[test]
    fn extension_ref_edgion_plugins_does_not_flag_rule() {
        // A valid EdgionPlugins ExtensionRef (group empty or edgion.io) must not be flagged.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-plugins
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        assert!(find_invalid_rule_details(&route, "default").is_empty());
    }

    #[test]
    fn extension_ref_unsupported_kind_on_backend_ref_marks_rule_invalid() {
        // An ExtensionRef filter with an unsupported kind on a backendRef must be
        // flagged, and the reason must carry the per-backendRef prefix.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - backendRefs:
        - name: svc
          port: 80
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: NotAPlugin
                name: x
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(
            indices_only(details.clone()),
            vec![0],
            "rule 0 must be flagged: {details:?}"
        );
        assert!(
            details[0].1.contains("backendRefs["),
            "reason must carry the per-backendRef prefix: {}",
            details[0].1
        );
    }

    #[test]
    fn extension_ref_wrong_group_with_edgion_plugins_kind_marks_rule_invalid() {
        // kind is EdgionPlugins but the group is wrong — must still be flagged,
        // proving the group_ok condition is actually ANDed in, not dead code.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: example.com
            kind: EdgionPlugins
            name: my-plugins
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(
            indices_only(details.clone()),
            vec![0],
            "rule 0 must be flagged: {details:?}"
        );
        assert!(
            details[0].1.contains("ExtensionRef") && details[0].1.contains("example.com"),
            "reason must name the wrong group: {}",
            details[0].1
        );
    }

    #[test]
    fn extension_ref_lb_policy_on_backend_ref_now_flagged_as_unsupported() {
        // The route-level extensionRef LB mechanism was removed: `LBPolicy*`
        // kinds are no longer a recognized second use of the ExtensionRef
        // filter type. A `LBPolicyConsistentHash` backendRef ExtensionRef now
        // fails the same "only EdgionPlugins is supported" gate as any other
        // unknown kind. Load-balancing policy is configured via
        // `EdgionBackendTrafficPolicy` instead.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - backendRefs:
        - name: svc
          port: 80
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyConsistentHash
                name: header.x-user-id
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(
            indices_only(details.clone()),
            vec![0],
            "LBPolicy backendRef ExtensionRef must now be flagged invalid: {details:?}"
        );
        assert!(
            details[0].1.contains("LBPolicyConsistentHash"),
            "reason must name the unsupported kind: {}",
            details[0].1
        );
    }

    #[test]
    fn extension_ref_lb_policy_at_rule_level_now_flagged_as_unsupported() {
        // Same removal as above, exercised at rule level (filters[]) instead of
        // backendRef level.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: LBPolicyConsistentHash
            name: header.x-user-id
      backendRefs:
        - name: svc
          port: 80
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(
            indices_only(details.clone()),
            vec![0],
            "rule-level LBPolicy ExtensionRef must now be flagged invalid: {details:?}"
        );
        assert!(
            details[0].1.contains("LBPolicyConsistentHash"),
            "reason must name the unsupported kind: {}",
            details[0].1
        );
    }

    #[test]
    fn rewrite_redirect_conflict_still_flags_with_reason() {
        // Regression: existing rewrite/redirect-conflict precedent still flags
        // the rule, now also carrying a reason string in the PartiallyInvalid
        // message rather than just an index list.
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules:
    - filters:
        - type: URLRewrite
          urlRewrite:
            path:
              type: ReplaceFullPath
              replaceFullPath: /a
        - type: RequestRedirect
          requestRedirect:
            scheme: https
"#;
        let route: HTTPRoute = serde_yaml::from_str(yaml).unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0]);
        assert!(
            details[0].1.contains("URLRewrite") && details[0].1.contains("RequestRedirect"),
            "rewrite/redirect reason text must mention both filters; got: {}",
            details[0].1
        );
    }
}

// Locks the dropped-parent-gateway requeue on the coalesced apply path; see
// `tests/coalesced_recreate_test.rs`.
#[cfg(test)]
#[path = "tests/coalesced_recreate_test.rs"]
mod coalesced_recreate_test;

//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::{is_cross_ns_ref_allowed, validate_http_route_if_enabled};
use super::route_delegation::{flatten_route, populate_terminal_rule_identities, DelegationLookup};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_backend_policies_for_backend_targets,
    requeue_backend_policies_for_route, requeue_backend_policies_for_services, requeue_parent_gateways,
    update_gateway_route_index,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, set_parent_conditions_full, update_condition,
    AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::{EdgionBackend, HTTPRoute};
use edgion_resources::resources::common::{ParentReference, RefDenied};
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::{AiProxyConfig, EdgionPlugin, EdgionPlugins};
use edgion_resources::resources::http_route::{
    logical_backend_slot_identities, validate_mcp_http_route_rule, DelegationIssue, HTTPBackendRefKind,
    HTTPRouteFilterType, HTTPRouteRule, HTTPRouteStatus, RouteParentStatus, GATEWAY_API_GROUP,
};
use edgion_resources::resources::plugin_policy::PluginPolicyAction;
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
    /// BackendRef filters deliberately do not participate: protocol gateway
    /// plugins are route-rule policies. Replacing the full ref set on every
    /// reconcile also handles delete-to-create coalescing.
    fn register_plugin_refs(route: &HTTPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().plugin_ref_mgr;
        manager.clear_resource_refs(&resource_ref);

        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        // Keep dependencies declared on the operator route as well as those
        // inherited by flattened child rules. This runs before protocol
        // filtering, so a withdrawn MCP rule can still be restored when its
        // PluginPolicy changes again.
        for rules in [route.spec.rules.as_deref(), route.spec.resolved_rules.as_deref()]
            .into_iter()
            .flatten()
        {
            for rule in rules {
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
    }

    /// Replace this route's EdgionBackend dependency set and return the
    /// old/new union plus the removed subset. The union is needed because an
    /// EBTP attached to either side may change the route's Gateway ancestors.
    fn register_backend_refs(
        route: &HTTPRoute,
        flattened_tuples: &[(Option<String>, Option<String>, Option<String>, String)],
        ctx: &HandlerContext,
    ) -> BackendRefDelta {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().backend_ref_mgr;
        let old: HashSet<String> = manager.clear_resource_refs(&resource_ref).into_iter().collect();
        let canonical_key = |group: Option<&str>, kind: Option<&str>, namespace: Option<&str>, name: &str| {
            if group == Some("edgion.io") && kind == Some("EdgionBackend") {
                Some(
                    PolicyTargetKey {
                        target_kind: PolicyTargetKind::EdgionBackend,
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
            // FlattenOutcome preserves each declaring child namespace and
            // every terminal typed ref, including missing, denied, and refs
            // from a rule later rejected by Controller-side AI validation.
            // Keep those dependencies so a target or ReferenceGrant change
            // can wake the route and rebuild the complete flattened result.
            flattened_tuples
                .iter()
                .filter_map(|(group, kind, namespace, name)| {
                    canonical_key(group.as_deref(), kind.as_deref(), namespace.as_deref(), name)
                })
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

        // Register before protocol filtering so a rule removed below retains
        // the dependency that can make it eligible again on a later reconcile.
        Self::register_plugin_refs(&route, _ctx);

        let has_delegation = route.spec.resolved_rules.is_some();
        if let Some(rules) = &mut route.spec.resolved_rules {
            let mut resolved_index = 0usize;
            rules.retain(|rule| {
                let reason = protocol_rule_invalid_reason_with_runtime(rule, route_ns, _ctx.runtime(), has_delegation);
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

        let backend_ref_delta = Self::register_backend_refs(&route, &delegation_outcome.backend_tuples, _ctx);
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
        let removed_backend_targets = ctx.runtime().backend_ref_mgr.clear_resource_refs(&resource_ref);
        requeue_backend_policies_for_backend_targets(&removed_backend_targets, ctx).await;
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

        // Protocol attachment and backend-contract violations are per-rule findings, like a
        // rewrite/redirect conflict: they drop just that rule via
        // `invalid_rule_indices` + PartiallyInvalid. They are lifted to a
        // route-level Accepted=False only when no rule is left to serve for this
        // parent (`protocol_rejects_route` below). Emitting both would advertise
        // "N of M rules active" while the gateway drops the entire route.
        let protocol_invalid_rules: Vec<(usize, String)> = route
            .spec
            .rules
            .iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, rule)| {
                protocol_rule_invalid_reason_with_runtime(
                    rule,
                    route_ns,
                    _ctx.runtime(),
                    route.spec.resolved_rules.is_some(),
                )
                .map(|message| (index, message))
            })
            .collect();

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

        // Nothing is left to serve for this parent when no rule survives: with
        // delegation the gateway compiles `resolved_rules` verbatim (already
        // pre-filtered during parse), otherwise it compiles `spec.rules` minus
        // `invalid_rule_indices`.
        let serves_no_rule = match &route.spec.resolved_rules {
            Some(resolved) => resolved.is_empty(),
            None => valid_rules == 0,
        };
        let protocol_rejects_route = !protocol_invalid_rules.is_empty() && serves_no_rule;

        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        if protocol_rejects_route {
            for (index, message) in &protocol_invalid_rules {
                validation_accepted_errors.push(AcceptedError::UnsupportedValue {
                    message: format!("rules[{index}]: {message}"),
                });
            }
        }

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
                //
                // The single exception is `protocol_rejects_route`: an AI/A2A violation that
                // leaves no rule serving at all. There the whole-route drop is the
                // intended outcome (AI-3), so Accepted=False carries the reasons and
                // PartiallyInvalid is never set True below (an earlier True is
                // cleared to False).

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
                // Accepted, matching the per-rule precedent above). Skipped when
                // `protocol_rejects_route` already rejected the whole route: a route
                // that serves nothing must not also claim rules are active.
                let own_invalid = !invalid_indices.is_empty() && valid_rules > 0;
                if !protocol_rejects_route && (own_invalid || !delegation_dropped.is_empty()) {
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
                    // Clear a stale True. A route rejected wholesale must not be
                    // cleared with the generic "all rules are valid" wording —
                    // that would contradict the Accepted=False sitting next to it.
                    let (reason, message) = if protocol_rejects_route {
                        (
                            condition_reasons::UNSUPPORTED_VALUE,
                            "Route not accepted; no rule is active",
                        )
                    } else {
                        (condition_reasons::ALL_RULES_VALID, "All rules are valid")
                    };
                    update_condition(
                        conditions,
                        condition_false(condition_types::PARTIALLY_INVALID, reason, message, generation),
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
                HTTPBackendRefKind::EdgionBackend => {
                    if backend_ref.port.is_some() {
                        errors.push(ResolvedRefsError::InvalidKind {
                            kind: "edgion.io/EdgionBackend with port".to_string(),
                            name: backend_ref.name.clone(),
                        });
                        continue;
                    }
                    let exists = runtime
                        .processor_registry
                        .get("EdgionBackend")
                        .is_none_or(|processor| processor.contains_key(&format!("{namespace}/{}", backend_ref.name)));
                    if !exists {
                        errors.push(ResolvedRefsError::EdgionBackendNotFound {
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct AiProxyAdmissionState {
    active_count: usize,
    policy_blocked: bool,
    config: Option<AiProxyConfig>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct A2aGatewayAdmissionState {
    active_count: usize,
    policy_blocked: bool,
    validation_failed: bool,
}

fn a2a_gateway_admission_state(
    runtime: &ControllerRuntime,
    namespace: &str,
    name: &str,
) -> Option<A2aGatewayAdmissionState> {
    let processor = runtime.processor_registry.get("EdgionPlugins")?;
    let key = format!("{namespace}/{name}");
    // Read the current raw generation: EdgionPlugins requeues dependent
    // routes before saving its processed generation, so ServerCache may still
    // contain the previous object during this reconcile.
    let raw = processor.get_raw_any(&key)?.downcast::<EdgionPlugins>().ok()?;
    let mut plugins = (*raw).clone();
    let _ = super::super::plugin_policy_stamp::stamp_http_entries(&mut plugins);
    let mut state = A2aGatewayAdmissionState::default();
    for entry in plugins.spec.request_plugins.iter().flatten() {
        if !entry.is_enabled() {
            continue;
        }
        match entry.policy_action.as_ref() {
            None if matches!(entry.plugin, EdgionPlugin::A2aGateway(_)) => state.active_count += 1,
            Some(PluginPolicyAction::Block { .. }) => state.policy_blocked = true,
            None | Some(PluginPolicyAction::Bypass) => {}
        }
    }
    macro_rules! stage_has_policy_block {
        ($entries:expr) => {
            $entries.iter().flatten().any(|entry| {
                entry.is_enabled()
                    && matches!(
                        entry.policy_action.as_ref(),
                        Some(PluginPolicyAction::Block { .. })
                    )
            })
        };
    }
    state.policy_blocked |= stage_has_policy_block!(plugins.spec.upstream_response_filter_plugins);
    state.policy_blocked |= stage_has_policy_block!(plugins.spec.upstream_response_body_filter_plugins);
    state.policy_blocked |= stage_has_policy_block!(plugins.spec.upstream_response_plugins);
    state.validation_failed = !plugins.validate_plugins().is_empty();
    Some(state)
}

fn ai_proxy_admission_state(runtime: &ControllerRuntime, namespace: &str, name: &str) -> Option<AiProxyAdmissionState> {
    let processor = runtime.processor_registry.get("EdgionPlugins")?;
    let key = format!("{namespace}/{name}");
    let plugins = processor.get_cached_any(&key)?.downcast::<EdgionPlugins>().ok()?;
    let mut state = AiProxyAdmissionState::default();
    for entry in plugins.spec.request_plugins.iter().flatten() {
        if !entry.is_enabled() || !matches!(entry.plugin, EdgionPlugin::AiProxy(_)) {
            if entry.is_enabled() && matches!(entry.policy_action.as_ref(), Some(PluginPolicyAction::Block { .. })) {
                state.policy_blocked = true;
            }
            continue;
        }
        match entry.policy_action.as_ref() {
            None => {
                state.active_count += 1;
                if let EdgionPlugin::AiProxy(config) = &entry.plugin {
                    state.config = Some(config.clone());
                }
            }
            Some(PluginPolicyAction::Bypass) => {}
            Some(PluginPolicyAction::Block { .. }) => state.policy_blocked = true,
        }
    }
    macro_rules! stage_has_policy_block {
        ($entries:expr) => {
            $entries.iter().flatten().any(|entry| {
                entry.is_enabled()
                    && matches!(
                        entry.policy_action.as_ref(),
                        Some(PluginPolicyAction::Block { .. })
                    )
            })
        };
    }
    state.policy_blocked |= stage_has_policy_block!(plugins.spec.upstream_response_filter_plugins);
    state.policy_blocked |= stage_has_policy_block!(plugins.spec.upstream_response_body_filter_plugins);
    state.policy_blocked |= stage_has_policy_block!(plugins.spec.upstream_response_plugins);
    Some(state)
}

/// Enforce the Controller-owned AI routing contract for one effective rule.
/// The lookup returns the active request-stage AiProxy count plus whether any
/// enabled entry in the referenced EdgionPlugins chain is policy-blocked, or
/// `None` when that object is unavailable.
#[cfg(test)]
fn ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    mut lookup: impl FnMut(&str, &str) -> Option<AiProxyAdmissionState>,
) -> Option<String> {
    let mut ai_proxy_count = 0usize;
    let mut ai_proxy_policy_blocked = false;
    let mut ai_proxy_configs = Vec::new();
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
            Some(state) => {
                ai_proxy_count += state.active_count;
                ai_proxy_policy_blocked |= state.policy_blocked;
                if let Some(config) = state.config {
                    ai_proxy_configs.push(config);
                }
            }
            None => missing_plugin = Some(format!("{namespace}/{}", extension_ref.name)),
        }
    }

    if ai_proxy_policy_blocked {
        // PluginPolicy Block is an unconditional request-stage gate for any
        // enabled entry in the referenced EdgionPlugins chain. Preserve the
        // rule before any AI admission return can remove that fail-close gate.
        return None;
    }

    let refs = rule.backend_refs.as_deref().unwrap_or_default();
    let identities = logical_backend_slot_identities(refs, route_namespace);
    let mut promoted_slots = HashSet::new();
    for config in &ai_proxy_configs {
        for model_route in &config.model_routes {
            let Some(candidates) = &model_route.action.candidates else {
                continue;
            };
            for candidate in candidates {
                let matches = identities
                    .iter()
                    .enumerate()
                    .filter(|(_, identity)| identity.matches_ai_selector(&candidate.backend))
                    .map(|(slot, _)| slot)
                    .collect::<Vec<_>>();
                let [slot] = matches.as_slice() else {
                    continue;
                };
                if candidate.weight.unwrap_or(refs[*slot].weight.unwrap_or(1)) > 0 {
                    promoted_slots.insert(*slot);
                }
            }
        }
    }
    let positive_backend_refs: Vec<_> = refs
        .iter()
        .enumerate()
        .filter(|(slot, backend_ref)| backend_ref.weight.unwrap_or(1) > 0 || promoted_slots.contains(slot))
        .map(|(_, backend_ref)| backend_ref)
        .collect();
    let ai_backends: Vec<_> = positive_backend_refs
        .iter()
        .copied()
        .filter(|backend_ref| backend_ref.backend_kind() == HTTPBackendRefKind::EdgionBackend)
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
        if ai_backends.len() != positive_backend_refs.len() {
            return Some("EdgionBackend backends must not be mixed with other backend kinds".to_string());
        }
        if ai_backends.iter().any(|backend_ref| backend_ref.port.is_some()) {
            return Some("EdgionBackend backendRefs must not set port".to_string());
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
                "EdgionBackend backendRefs do not support {:?} filters",
                filter.filter_type
            ));
        }
    }

    if ai_backends.is_empty() {
        return (ai_proxy_count > 0)
            .then(|| "AiProxy requires a rule whose forwarding backends are all EdgionBackend".to_string());
    }
    if let Some(key) = missing_plugin {
        return Some(format!(
            "AI routing requires exactly one enabled AiProxy, but EdgionPlugins '{key}' was not found"
        ));
    }
    (ai_proxy_count != 1).then(|| format!("AI routing requires exactly one enabled AiProxy (found {ai_proxy_count})"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForwardingBackendClass {
    Service,
    Ai,
    A2a,
    Mcp,
    Missing,
    Denied,
    Invalid,
}

fn runtime_ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    let mut ai_proxy_count = 0usize;
    let mut missing_plugin = None;
    let mut configs = Vec::new();
    for filter in rule.filters.iter().flatten() {
        if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
            continue;
        }
        let Some(reference) = filter.extension_ref.as_ref() else {
            continue;
        };
        if !(reference.group.is_empty() || reference.group == "edgion.io") || reference.kind != "EdgionPlugins" {
            continue;
        }
        let namespace = reference.resolved_namespace.as_deref().unwrap_or(route_namespace);
        match ai_proxy_admission_state(runtime, namespace, &reference.name) {
            Some(state) => {
                ai_proxy_count += state.active_count;
                if let Some(config) = state.config {
                    configs.push(config);
                }
            }
            None => missing_plugin = Some(format!("{namespace}/{}", reference.name)),
        }
    }

    let refs = rule.backend_refs.as_deref().unwrap_or_default();
    let identities = logical_backend_slot_identities(refs, route_namespace);
    let mut promoted_slots = HashSet::new();
    for config in &configs {
        for model_route in &config.model_routes {
            for candidate in model_route.action.candidates.iter().flatten() {
                let matches = identities
                    .iter()
                    .enumerate()
                    .filter(|(_, identity)| identity.matches_ai_selector(&candidate.backend))
                    .map(|(slot, _)| slot)
                    .collect::<Vec<_>>();
                if let [slot] = matches.as_slice() {
                    if candidate.weight.unwrap_or(refs[*slot].weight.unwrap_or(1)) > 0 {
                        promoted_slots.insert(*slot);
                    }
                }
            }
        }
    }

    let backend_processor = runtime.processor_registry.get("EdgionBackend");
    let mut classes = Vec::new();
    for (slot, backend_ref) in refs
        .iter()
        .enumerate()
        .filter(|(slot, backend_ref)| backend_ref.weight.unwrap_or(1) > 0 || promoted_slots.contains(slot))
    {
        let class = if backend_ref.ref_denied.is_some() {
            ForwardingBackendClass::Denied
        } else {
            match backend_ref.backend_kind() {
                HTTPBackendRefKind::Service => {
                    let namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
                    if super::route_utils::service_exists(namespace, &backend_ref.name, runtime) {
                        ForwardingBackendClass::Service
                    } else {
                        ForwardingBackendClass::Missing
                    }
                }
                HTTPBackendRefKind::EdgionBackend if backend_ref.port.is_some() => ForwardingBackendClass::Invalid,
                HTTPBackendRefKind::EdgionBackend => {
                    let namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
                    let key = format!("{namespace}/{}", backend_ref.name);
                    match backend_processor.as_ref() {
                        // The registry is always populated in a running Controller. Isolated
                        // admission tests may omit it; treat that as missing instead of guessing
                        // an AI or A2A variant without the current raw resource generation.
                        None => ForwardingBackendClass::Missing,
                        Some(processor) => match processor
                            .get_raw_any(&key)
                            .and_then(|raw| raw.downcast::<EdgionBackend>().ok())
                        {
                            Some(backend) if backend.spec.validate().is_err() => ForwardingBackendClass::Invalid,
                            Some(backend)
                                if matches!(
                                    (
                                        backend.spec.ai.is_some(),
                                        backend.spec.a2a.is_some(),
                                        backend.spec.mcp.is_some()
                                    ),
                                    (true, false, false)
                                ) =>
                            {
                                ForwardingBackendClass::Ai
                            }
                            Some(backend)
                                if matches!(
                                    (
                                        backend.spec.ai.is_some(),
                                        backend.spec.a2a.is_some(),
                                        backend.spec.mcp.is_some()
                                    ),
                                    (false, true, false)
                                ) =>
                            {
                                ForwardingBackendClass::A2a
                            }
                            Some(backend)
                                if matches!(
                                    (
                                        backend.spec.ai.is_some(),
                                        backend.spec.a2a.is_some(),
                                        backend.spec.mcp.is_some()
                                    ),
                                    (false, false, true)
                                ) =>
                            {
                                ForwardingBackendClass::Mcp
                            }
                            Some(_) => ForwardingBackendClass::Invalid,
                            None => ForwardingBackendClass::Missing,
                        },
                    }
                }
                HTTPBackendRefKind::RouteDelegation | HTTPBackendRefKind::Unsupported => {
                    ForwardingBackendClass::Invalid
                }
            }
        };
        classes.push((slot, backend_ref, class));
    }

    let has_ai = classes.iter().any(|(_, _, class)| *class == ForwardingBackendClass::Ai);
    if !has_ai {
        if classes
            .iter()
            .any(|(_, _, class)| *class == ForwardingBackendClass::Invalid)
        {
            return Some("Forwarding backend has an invalid kind or variant".to_string());
        }
        return (ai_proxy_count > 0)
            .then(|| "AiProxy requires a rule with positive AI EdgionBackend slots".to_string());
    }

    if rule
        .retry
        .as_ref()
        .and_then(|retry| retry.attempts)
        .is_some_and(|attempts| attempts > 7)
    {
        return Some("AI routing retry.attempts must not exceed 7".to_string());
    }
    if classes.iter().any(|(_, _, class)| {
        matches!(
            class,
            ForwardingBackendClass::Service
                | ForwardingBackendClass::A2a
                | ForwardingBackendClass::Mcp
                | ForwardingBackendClass::Invalid
        )
    }) {
        return Some("AI EdgionBackend slots must not be mixed with other forwarding backend variants".to_string());
    }
    for (_, backend_ref, class) in &classes {
        if *class != ForwardingBackendClass::Ai {
            continue;
        }
        if backend_ref.port.is_some() {
            return Some("EdgionBackend backendRefs must not set port".to_string());
        }
        if let Some(filter) = backend_ref.filters.iter().flatten().find(|filter| {
            !matches!(
                filter.filter_type,
                HTTPRouteFilterType::RequestHeaderModifier | HTTPRouteFilterType::ResponseHeaderModifier
            )
        }) {
            return Some(format!(
                "EdgionBackend backendRefs do not support {:?} filters",
                filter.filter_type
            ));
        }
    }
    if let Some(key) = missing_plugin {
        return Some(format!(
            "AI routing requires exactly one enabled AiProxy, but EdgionPlugins '{key}' was not found"
        ));
    }
    (ai_proxy_count != 1).then(|| format!("AI routing requires exactly one enabled AiProxy (found {ai_proxy_count})"))
}

fn runtime_a2a_rule_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    let mut a2a_gateway_count = 0usize;
    let mut policy_blocked = false;
    let mut plugin_validation_failed = false;
    let mut missing_plugin = None;
    for filter in rule.filters.iter().flatten() {
        if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
            continue;
        }
        let Some(reference) = filter.extension_ref.as_ref() else {
            continue;
        };
        if !(reference.group.is_empty() || reference.group == "edgion.io") || reference.kind != "EdgionPlugins" {
            continue;
        }
        let namespace = reference.resolved_namespace.as_deref().unwrap_or(route_namespace);
        match a2a_gateway_admission_state(runtime, namespace, &reference.name) {
            Some(state) => {
                a2a_gateway_count += state.active_count;
                policy_blocked |= state.policy_blocked;
                plugin_validation_failed |= state.validation_failed;
            }
            None => missing_plugin = Some(format!("{namespace}/{}", reference.name)),
        }
    }

    // A policy Block is a fail-close request gate. Removing this rule would
    // remove the gate, so it always wins over A2A attachment validation.
    if policy_blocked {
        return None;
    }
    if a2a_gateway_count > 0 && plugin_validation_failed {
        return Some("A2aGateway references an invalid EdgionPlugins generation".to_string());
    }

    let backend_refs = rule.backend_refs.as_deref().unwrap_or_default();
    let backend_processor = runtime.processor_registry.get("EdgionBackend");
    let mut classes = Vec::new();
    for backend_ref in backend_refs
        .iter()
        .filter(|backend_ref| backend_ref.weight.unwrap_or(1) > 0)
    {
        let class = if backend_ref.ref_denied.is_some() {
            ForwardingBackendClass::Denied
        } else {
            match backend_ref.backend_kind() {
                // A2A eligibility is typed by the syntactic backend kind. A
                // missing core Service must not be reclassified as an allowed
                // missing EdgionBackend share: the Gateway cannot prove that
                // it was intended to become an A2A variant later.
                HTTPBackendRefKind::Service => ForwardingBackendClass::Service,
                HTTPBackendRefKind::EdgionBackend if backend_ref.port.is_some() => ForwardingBackendClass::Invalid,
                HTTPBackendRefKind::EdgionBackend => {
                    let namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
                    let key = format!("{namespace}/{}", backend_ref.name);
                    match backend_processor.as_ref() {
                        None => ForwardingBackendClass::Missing,
                        Some(processor) => match processor
                            .get_raw_any(&key)
                            .and_then(|raw| raw.downcast::<EdgionBackend>().ok())
                        {
                            Some(backend) if backend.spec.validate().is_err() => ForwardingBackendClass::Invalid,
                            Some(backend)
                                if matches!(
                                    (
                                        backend.spec.ai.is_some(),
                                        backend.spec.a2a.is_some(),
                                        backend.spec.mcp.is_some()
                                    ),
                                    (true, false, false)
                                ) =>
                            {
                                ForwardingBackendClass::Ai
                            }
                            Some(backend)
                                if matches!(
                                    (
                                        backend.spec.ai.is_some(),
                                        backend.spec.a2a.is_some(),
                                        backend.spec.mcp.is_some()
                                    ),
                                    (false, true, false)
                                ) =>
                            {
                                ForwardingBackendClass::A2a
                            }
                            Some(backend)
                                if matches!(
                                    (
                                        backend.spec.ai.is_some(),
                                        backend.spec.a2a.is_some(),
                                        backend.spec.mcp.is_some()
                                    ),
                                    (false, false, true)
                                ) =>
                            {
                                ForwardingBackendClass::Mcp
                            }
                            Some(_) => ForwardingBackendClass::Invalid,
                            None => ForwardingBackendClass::Missing,
                        },
                    }
                }
                HTTPBackendRefKind::RouteDelegation | HTTPBackendRefKind::Unsupported => {
                    ForwardingBackendClass::Invalid
                }
            }
        };
        classes.push(class);
    }

    // Ordinary WP4 A2A backend routes remain valid without the gateway
    // protocol plugin. This classifier owns only rules that actually resolve
    // an active route-level A2aGateway.
    if a2a_gateway_count == 0 {
        return None;
    }
    if let Some(key) = missing_plugin {
        return Some(format!(
            "A2A routing requires exactly one enabled A2aGateway, but EdgionPlugins '{key}' was not found"
        ));
    }
    if a2a_gateway_count != 1 {
        return Some(format!(
            "A2A routing requires exactly one enabled A2aGateway (found {a2a_gateway_count})"
        ));
    }

    if backend_refs.is_empty() {
        let matches = rule.matches.as_deref().unwrap_or_default();
        let valid_discovery_match = |route_match: &edgion_resources::resources::http_route::HTTPRouteMatch| {
            route_match.method.as_deref() == Some("GET")
                && route_match.path.as_ref().is_some_and(|path| {
                    path.match_type.as_deref() == Some("Exact")
                        && path.value.as_deref() == Some("/.well-known/agent-card.json")
                })
        };
        if matches.is_empty() || !matches.iter().all(valid_discovery_match) {
            return Some(
                "A2aGateway backendless rules require explicit GET Exact '/.well-known/agent-card.json' matches"
                    .to_string(),
            );
        }
        return None;
    }

    if classes.is_empty() {
        return Some("A2aGateway proxy rules require at least one positive-weight backendRef".to_string());
    }

    if classes.iter().any(|class| {
        matches!(
            class,
            ForwardingBackendClass::Service
                | ForwardingBackendClass::Ai
                | ForwardingBackendClass::Mcp
                | ForwardingBackendClass::Invalid
        )
    }) {
        return Some(
            "A2aGateway proxy rules require positive existing forwarding backends to use EdgionBackend spec.a2a"
                .to_string(),
        );
    }
    None
}

fn backend_variant_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    let processor = runtime.processor_registry.get("EdgionBackend")?;
    for backend_ref in rule.backend_refs.iter().flatten().filter(|backend_ref| {
        backend_ref.weight.unwrap_or(1) > 0
            && backend_ref.ref_denied.is_none()
            && backend_ref.backend_kind() == HTTPBackendRefKind::EdgionBackend
    }) {
        let namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
        let key = format!("{namespace}/{}", backend_ref.name);
        let Some(raw) = processor.get_raw_any(&key) else {
            continue;
        };
        let Ok(backend) = raw.downcast::<EdgionBackend>() else {
            return Some("EdgionBackend target has an incompatible cached representation".to_string());
        };
        if backend.spec.validate().is_err() {
            return Some("EdgionBackend target does not configure a supported variant".to_string());
        }
    }
    None
}

fn active_ai_proxy_config(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<AiProxyConfig> {
    rule.filters.iter().flatten().find_map(|filter| {
        if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
            return None;
        }
        let reference = filter.extension_ref.as_ref()?;
        if !(reference.group.is_empty() || reference.group == "edgion.io") || reference.kind != "EdgionPlugins" {
            return None;
        }
        let namespace = reference.resolved_namespace.as_deref().unwrap_or(route_namespace);
        ai_proxy_admission_state(runtime, namespace, &reference.name)?.config
    })
}

fn model_route_context_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    let config = active_ai_proxy_config(rule, route_namespace, runtime)?;
    model_route_context_invalid_reason_for_config(rule, route_namespace, runtime, &config)
}

fn model_route_context_invalid_reason_for_config(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
    config: &AiProxyConfig,
) -> Option<String> {
    if config.model_routes.is_empty() {
        return None;
    }
    let refs = rule.backend_refs.as_deref().unwrap_or_default();
    let identities = logical_backend_slot_identities(refs, route_namespace);
    let backend_processor = runtime.processor_registry.get("EdgionBackend")?;

    for (route_index, model_route) in config.model_routes.iter().enumerate() {
        let selected_slots = if let Some(candidates) = &model_route.action.candidates {
            let mut slots = Vec::with_capacity(candidates.len());
            for (candidate_index, candidate) in candidates.iter().enumerate() {
                let matches = identities
                    .iter()
                    .enumerate()
                    .filter(|(_, identity)| identity.matches_ai_selector(&candidate.backend))
                    .map(|(slot, _)| slot)
                    .collect::<Vec<_>>();
                let [slot] = matches.as_slice() else {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}] must resolve to one backendRef slot"
                    ));
                };
                let backend_ref = &refs[*slot];
                if backend_ref.ref_denied.is_some() {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}] selects a denied backendRef"
                    ));
                }
                if backend_ref.backend_kind() != HTTPBackendRefKind::EdgionBackend || backend_ref.port.is_some() {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}] must select an EdgionBackend slot"
                    ));
                }
                slots.push((*slot, candidate.weight.unwrap_or(backend_ref.weight.unwrap_or(1))));
            }
            slots
        } else {
            refs.iter()
                .enumerate()
                .filter(|(_, backend_ref)| {
                    backend_ref.ref_denied.is_none()
                        && backend_ref.backend_kind() == HTTPBackendRefKind::EdgionBackend
                        && backend_ref.port.is_none()
                })
                .map(|(slot, backend_ref)| (slot, backend_ref.weight.unwrap_or(1)))
                .collect()
        };

        let mut existing_selected = false;
        let mut rewrite_supported = model_route.action.rewrite.is_none();
        for (slot, effective_weight) in selected_slots {
            let backend_ref = &refs[slot];
            let namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
            let key = format!("{namespace}/{}", backend_ref.name);
            let Some(raw) = backend_processor.get_raw_any(&key) else {
                continue;
            };
            let Ok(backend) = raw.downcast::<EdgionBackend>() else {
                return Some("modelRoutes candidate has an incompatible cached backend representation".to_string());
            };
            let Some(ai) = backend.spec.ai.as_ref() else {
                return Some(format!(
                    "modelRoutes[{route_index}] selects an EdgionBackend without spec.ai"
                ));
            };
            if effective_weight <= 0 {
                continue;
            }
            existing_selected = true;
            if let Some(rewrite) = &model_route.action.rewrite {
                rewrite_supported |= ai.models.iter().any(|model| model.name == *rewrite);
            }
        }
        if existing_selected && !rewrite_supported {
            return Some(format!(
                "modelRoutes[{route_index}].action.rewrite must name a canonical model on a selected backend"
            ));
        }
    }
    None
}

fn plugin_policy_blocks_rule(rule: &HTTPRouteRule, route_namespace: &str, runtime: &ControllerRuntime) -> bool {
    rule.filters.iter().flatten().any(|filter| {
        if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
            return false;
        }
        let Some(extension_ref) = &filter.extension_ref else {
            return false;
        };
        if !(extension_ref.group.is_empty() || extension_ref.group == "edgion.io")
            || extension_ref.kind != "EdgionPlugins"
        {
            return false;
        }
        let namespace = extension_ref.resolved_namespace.as_deref().unwrap_or(route_namespace);
        a2a_gateway_admission_state(runtime, namespace, &extension_ref.name).is_some_and(|state| state.policy_blocked)
    })
}

fn runtime_mcp_rule_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
    has_delegation: bool,
) -> Option<String> {
    let processor = runtime.processor_registry.get("EdgionBackend")?;
    let has_current_mcp_backend = rule
        .backend_refs
        .iter()
        .flatten()
        .filter(|backend_ref| {
            backend_ref.ref_denied.is_none() && backend_ref.backend_kind() == HTTPBackendRefKind::EdgionBackend
        })
        .any(|backend_ref| {
            let namespace = backend_ref.namespace.as_deref().unwrap_or(route_namespace);
            let key = format!("{namespace}/{}", backend_ref.name);
            processor
                .get_raw_any(&key)
                .and_then(|raw| raw.downcast::<EdgionBackend>().ok())
                .is_some_and(|backend| backend.spec.validate().is_ok() && backend.spec.mcp.is_some())
        });
    if !has_current_mcp_backend {
        return None;
    }
    if has_delegation {
        return Some("MCP rules do not support HTTPRoute delegation".to_string());
    }
    validate_mcp_http_route_rule(rule).err().map(|error| error.to_string())
}

fn protocol_rule_invalid_reason_with_runtime(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
    has_delegation: bool,
) -> Option<String> {
    protocol_rule_invalid_reason_after_policy(
        rule,
        route_namespace,
        runtime,
        has_delegation,
        plugin_policy_blocks_rule(rule, route_namespace, runtime),
    )
}

fn protocol_rule_invalid_reason_after_policy(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
    has_delegation: bool,
    policy_blocked: bool,
) -> Option<String> {
    // A PluginPolicy Block is a fail-close request gate. No backend admission
    // finding may withdraw the rule and thereby remove that gate.
    if policy_blocked {
        return None;
    }
    runtime_ai_rule_invalid_reason(rule, route_namespace, runtime)
        .or_else(|| runtime_a2a_rule_invalid_reason(rule, route_namespace, runtime))
        .or_else(|| runtime_mcp_rule_invalid_reason(rule, route_namespace, runtime, has_delegation))
        .or_else(|| backend_variant_invalid_reason(rule, route_namespace, runtime))
        .or_else(|| model_route_context_invalid_reason(rule, route_namespace, runtime))
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
        if let Some(reason) =
            protocol_rule_invalid_reason_with_runtime(rule, route_ns, runtime, route.spec.resolved_rules.is_some())
        {
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

    fn make_ctx_with_raw_ai_backend(namespace: &str, name: &str) -> HandlerContext {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

        let backend: EdgionBackend = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"namespace": namespace, "name": name},
            "spec": {"ai": {
                "provider": "OpenAI",
                "credentialPool": {"credentials": [{
                    "name": "primary", "secretRef": {"name": "provider-key"}
                }]},
                "models": [{"name": "model-a"}]
            }}
        }))
        .unwrap();
        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert(format!("{namespace}/{name}"), Arc::new(backend));
        processor.set_raw_store(raw);
        runtime.processor_registry.register(processor);
        HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
    }

    fn a2a_rule(backends: serde_json::Value, matches: serde_json::Value) -> HTTPRouteRule {
        serde_json::from_value(serde_json::json!({
            "matches": matches,
            "filters": [{
                "type": "ExtensionRef",
                "extensionRef": {
                    "group": "edgion.io",
                    "kind": "EdgionPlugins",
                    "name": "a2a"
                }
            }],
            "backendRefs": backends
        }))
        .expect("A2A rule fixture must deserialize")
    }

    fn valid_a2a_card() -> serde_json::Value {
        serde_json::json!({
            "name": "Test agent",
            "description": "Agent used by controller admission tests",
            "version": "1.0.0",
            "capabilities": {},
            "supportedInterfaces": [{
                "url": "https://agent.example.test/rpc",
                "protocolBinding": "JSONRPC",
                "protocolVersion": "1.0"
            }],
            "defaultInputModes": ["application/json"],
            "defaultOutputModes": ["application/json"],
            "skills": [{
                "id": "echo",
                "name": "Echo",
                "description": "Echo a test message",
                "tags": ["test"]
            }]
        })
    }

    fn runtime_with_a2a_plugin(raw_backends: &[(&str, serde_json::Value)]) -> Arc<ControllerRuntime> {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::{EdgionBackendHandler, EdgionPluginsHandler};
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_plugins::{A2aGatewayConfig, EdgionPluginsSpec, RequestFilterEntry};

        let runtime = Arc::new(ControllerRuntime::default());
        let plugin_processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::default()),
            runtime.clone(),
        ));
        let mut plugins = EdgionPlugins::new(
            "a2a",
            EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::A2aGateway(
                    A2aGatewayConfig {
                        card: valid_a2a_card(),
                        external_url: None,
                        fail_open: false,
                    },
                ))]),
                ..Default::default()
            },
        );
        plugins.metadata.namespace = Some("apps".to_string());
        let plugin_raw_store = Arc::new(LocalRawStore::new());
        plugin_raw_store.upsert("apps/a2a".to_string(), Arc::new(plugins.clone()));
        plugin_processor.set_raw_store(plugin_raw_store);
        plugin_processor.save(plugins, None);
        runtime.processor_registry.register(plugin_processor);

        let backend_processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let raw_store = Arc::new(LocalRawStore::new());
        for (name, spec) in raw_backends {
            let backend: EdgionBackend = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionBackend",
                "metadata": {"namespace": "apps", "name": name},
                "spec": spec
            }))
            .expect("backend fixture must deserialize");
            raw_store.upsert(format!("apps/{name}"), Arc::new(backend));
        }
        backend_processor.set_raw_store(raw_store);
        runtime.processor_registry.register(backend_processor);
        runtime
    }

    #[tokio::test]
    async fn ai_rule_preserves_a_missing_service_as_an_invalid_share() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::{EdgionPluginsHandler, ServiceHandler};
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_plugins::{AiProxyConfig, EdgionPluginsSpec, RequestFilterEntry};
        use k8s_openapi::api::core::v1::Service;

        let runtime = runtime_with_a2a_plugin(&[(
            "models",
            serde_json::json!({
                "ai": {
                    "provider": "OpenAI",
                    "credentialPool": {"credentials": [{
                        "name": "primary", "secretRef": {"name": "provider-key"}
                    }]},
                    "models": [{"name": "model-a"}]
                }
            }),
        )]);
        let service_processor = Arc::new(ResourceProcessor::<Service>::new(
            "Service",
            16,
            Arc::new(ServiceHandler::new()),
            runtime.clone(),
        ));
        service_processor.set_raw_store(Arc::new(LocalRawStore::new()));
        runtime.processor_registry.register(service_processor);
        let plugin_processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::default()),
            runtime.clone(),
        ));
        let mut plugins = EdgionPlugins::new(
            "ai",
            EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::AiProxy(
                    serde_json::from_value::<AiProxyConfig>(serde_json::json!({}))
                        .expect("empty AiProxy config must use stable defaults"),
                ))]),
                ..Default::default()
            },
        );
        plugins.metadata.namespace = Some("apps".to_string());
        let plugin_raw_store = Arc::new(LocalRawStore::new());
        plugin_raw_store.upsert("apps/ai".to_string(), Arc::new(plugins.clone()));
        plugin_processor.set_raw_store(plugin_raw_store);
        plugin_processor.save(plugins, None);
        runtime.processor_registry.register(plugin_processor);

        let rule = ai_rule(
            serde_json::json!([
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "models"},
                {"name": "missing-service", "port": 80}
            ]),
            serde_json::json!([{
                "type": "ExtensionRef",
                "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
                }
            }]),
        );

        assert_eq!(runtime_ai_rule_invalid_reason(&rule, "apps", &runtime), None);
    }

    #[tokio::test]
    async fn a2a_local_card_requires_backendless_exact_get_matches() {
        let runtime = runtime_with_a2a_plugin(&[]);
        let exact_get = serde_json::json!([{
            "method": "GET",
            "path": {"type": "Exact", "value": "/.well-known/agent-card.json"}
        }]);
        assert!(
            runtime_a2a_rule_invalid_reason(&a2a_rule(serde_json::json!([]), exact_get), "apps", &runtime).is_none()
        );

        for matches in [
            serde_json::json!([{
                "method": "POST",
                "path": {"type": "Exact", "value": "/.well-known/agent-card.json"}
            }]),
            serde_json::json!([{
                "method": "GET",
                "path": {"type": "PathPrefix", "value": "/.well-known/agent-card.json"}
            }]),
            serde_json::json!([]),
        ] {
            let reason = runtime_a2a_rule_invalid_reason(&a2a_rule(serde_json::json!([]), matches), "apps", &runtime)
                .expect("loose local-card match must be rejected");
            assert!(reason.contains("explicit GET Exact"), "{reason}");
        }
    }

    #[tokio::test]
    async fn a2a_proxy_uses_current_raw_variant_and_preserves_invalid_shares() {
        let runtime = runtime_with_a2a_plugin(&[
            (
                "agent",
                serde_json::json!({
                    "a2a": {"target": {"endpoint": "https://agent.example.test"}}
                }),
            ),
            (
                "model",
                serde_json::json!({
                    "ai": {
                        "provider": "OpenAI",
                        "credentialPool": {"credentials": [{
                            "name": "primary", "secretRef": {"name": "provider-key"}
                        }]},
                        "models": [{"name": "model-a"}]
                    }
                }),
            ),
            (
                "tools",
                serde_json::json!({
                    "mcp": {
                        "target": {"endpoint": "https://tools.example.test/mcp"},
                        "transport": {"type": "StreamableHTTP"}
                    }
                }),
            ),
        ]);
        let rpc_matches = serde_json::json!([{"method": "POST"}]);
        let backend = |name: &str, extra: serde_json::Value| {
            let mut value = serde_json::json!({
                "group": "edgion.io", "kind": "EdgionBackend", "name": name
            });
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            value
        };

        let accepted = a2a_rule(
            serde_json::json!([
                backend("agent", serde_json::json!({})),
                backend("missing", serde_json::json!({})),
                backend("model", serde_json::json!({"weight": 0})),
                backend(
                    "model",
                    serde_json::json!({
                        "refDenied": {"targetNamespace": "apps", "targetName": "model"}
                    })
                )
            ]),
            rpc_matches.clone(),
        );
        assert!(runtime_a2a_rule_invalid_reason(&accepted, "apps", &runtime).is_none());

        let denied_service_share = a2a_rule(
            serde_json::json!([
                backend("agent", serde_json::json!({})),
                {
                    "name": "service",
                    "port": 80,
                    "refDenied": {"targetNamespace": "apps", "targetName": "service"}
                }
            ]),
            rpc_matches.clone(),
        );
        assert!(
            runtime_a2a_rule_invalid_reason(&denied_service_share, "apps", &runtime).is_none(),
            "a denied Service is an invalid share and must not affect A2A kind admission"
        );

        for (label, backends) in [
            ("AI", serde_json::json!([backend("model", serde_json::json!({}))])),
            ("MCP", serde_json::json!([backend("tools", serde_json::json!({}))])),
            (
                "mixed",
                serde_json::json!([
                    backend("agent", serde_json::json!({})),
                    backend("model", serde_json::json!({}))
                ]),
            ),
            ("missing Service", serde_json::json!([{"name": "service", "port": 80}])),
        ] {
            assert!(
                runtime_a2a_rule_invalid_reason(&a2a_rule(backends, rpc_matches.clone()), "apps", &runtime).is_some(),
                "{label} backend must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn a2a_proxy_rejects_nonempty_all_zero_backend_refs() {
        let runtime = runtime_with_a2a_plugin(&[(
            "agent",
            serde_json::json!({
                "a2a": {"target": {"endpoint": "https://agent.example.test"}}
            }),
        )]);
        let rule = a2a_rule(
            serde_json::json!([{
                "group": "edgion.io",
                "kind": "EdgionBackend",
                "name": "agent",
                "weight": 0
            }]),
            serde_json::json!([{"method": "POST"}]),
        );

        let reason = runtime_a2a_rule_invalid_reason(&rule, "apps", &runtime)
            .expect("an all-zero proxy rule has no forwarding slot");
        assert!(reason.contains("at least one positive-weight backendRef"), "{reason}");
    }

    #[tokio::test]
    async fn a2a_admission_uses_current_raw_plugin_generation() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_plugins::{
            A2aGatewayConfig, EdgionPluginsSpec, MockConfig, RequestFilterEntry,
        };

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::default()),
            runtime.clone(),
        ));
        let mut processed = EdgionPlugins::new(
            "a2a",
            EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::Mock(MockConfig::default()))]),
                ..Default::default()
            },
        );
        processed.metadata.namespace = Some("apps".to_string());
        processor.save(processed, None);

        let mut current = EdgionPlugins::new(
            "a2a",
            EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::A2aGateway(
                    A2aGatewayConfig {
                        card: valid_a2a_card(),
                        external_url: None,
                        fail_open: false,
                    },
                ))]),
                ..Default::default()
            },
        );
        current.metadata.namespace = Some("apps".to_string());
        let raw_store = Arc::new(LocalRawStore::new());
        raw_store.upsert("apps/a2a".to_string(), Arc::new(current));
        processor.set_raw_store(raw_store);
        runtime.processor_registry.register(processor);

        assert_eq!(
            a2a_gateway_admission_state(&runtime, "apps", "a2a"),
            Some(A2aGatewayAdmissionState {
                active_count: 1,
                policy_blocked: false,
                validation_failed: false,
            })
        );
    }

    #[tokio::test]
    async fn invalid_only_a2a_rule_sets_accepted_false() {
        let runtime = runtime_with_a2a_plugin(&[]);
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime);
        let mut route: HTTPRoute = serde_yaml::from_str(
            r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: card
  namespace: apps
spec:
  parentRefs:
    - name: gw
  rules:
    - matches:
        - method: POST
          path:
            type: Exact
            value: /.well-known/agent-card.json
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: a2a
"#,
        )
        .expect("A2A route fixture must deserialize");

        HttpRouteHandler::default().update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().expect("status").parents[0].conditions;
        let accepted = conditions
            .iter()
            .find(|condition| condition.type_ == condition_types::ACCEPTED)
            .expect("Accepted condition must be set");
        assert_eq!(accepted.status, "False");
        assert!(accepted.message.contains("rules[0]"), "{accepted:?}");
        assert!(accepted.message.contains("GET Exact"), "{accepted:?}");
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
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

    /// Build a route whose first rule is an AI rule violating the retry cap,
    /// optionally followed by a plain Service rule that stays valid.
    fn route_with_bad_ai_rule(with_valid_second_rule: bool) -> HTTPRoute {
        let second_rule = if with_valid_second_rule {
            r#"
    - backendRefs:
        - name: web
          port: 80
"#
        } else {
            ""
        };
        let yaml = format!(
            r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  parentRefs:
    - name: gw
  rules:
    - backendRefs:
        - group: edgion.io
          kind: EdgionBackend
          name: chat
      retry:
        attempts: 8
{second_rule}"#
        );
        serde_yaml::from_str(&yaml).expect("HTTPRoute yaml must parse")
    }

    #[tokio::test]
    async fn ai_invalid_rule_stays_partially_invalid_when_other_rules_survive() {
        // Contract: an AI violation is a per-rule finding. While any rule still
        // serves, it must NOT be lifted into the route-level Accepted error —
        // otherwise the gateway drops the whole route (filter_accepted_parent_refs)
        // while PartiallyInvalid simultaneously claims "N of M rules active".
        let mut route = route_with_bad_ai_rule(true);
        let ctx = make_ctx_with_raw_ai_backend("default", "chat");
        HttpRouteHandler::default().update_status(&mut route, &ctx, &[]);

        let conds = &route.status.as_ref().expect("status").parents[0].conditions;
        // The parent Gateway 'gw' is absent from the test runtime, so Accepted is
        // False for NoMatchingParent regardless; what matters is that the AI
        // reason was not merged into it.
        let accepted = conds.iter().find(|c| c.type_ == "Accepted").expect("Accepted set");
        assert!(
            !accepted.message.contains("retry.attempts"),
            "a bad AI rule must not reject the whole route while other rules serve; got: {accepted:?}"
        );

        let partial = conds
            .iter()
            .find(|c| c.type_ == "PartiallyInvalid")
            .expect("PartiallyInvalid set");
        assert_eq!(partial.status, "True");
        assert_eq!(partial.reason, "RuleInvalid");
        assert!(
            partial.message.contains("(1 of 2 rules active)"),
            "PartiallyInvalid must report the surviving rule count; got: {partial:?}"
        );
        assert_eq!(
            route.spec.invalid_rule_indices.as_deref(),
            Some(&[0usize][..]),
            "only the AI rule is dropped from the data plane"
        );
    }

    #[tokio::test]
    async fn ai_invalid_rule_rejects_route_only_when_nothing_serves() {
        // Contract: when the AI violation leaves no rule serving, the whole-route
        // rejection is the intended outcome (AI-3) — Accepted=False carries the
        // per-rule reason and PartiallyInvalid stays silent, so the two conditions
        // never contradict each other.
        let mut route = route_with_bad_ai_rule(false);
        let ctx = make_ctx_with_raw_ai_backend("default", "chat");
        HttpRouteHandler::default().update_status(&mut route, &ctx, &[]);

        let conds = &route.status.as_ref().expect("status").parents[0].conditions;
        let accepted = conds.iter().find(|c| c.type_ == "Accepted").expect("Accepted set");
        assert_eq!(accepted.status, "False");
        assert!(
            accepted.message.contains("rules[0]") && accepted.message.contains("retry.attempts"),
            "Accepted message must carry the per-rule AI reason; got: {accepted:?}"
        );
        assert!(
            conds
                .iter()
                .find(|c| c.type_ == "PartiallyInvalid")
                .is_none_or(|c| c.status == "False"),
            "a route that serves nothing must not also advertise active rules; got: {conds:?}"
        );
    }

    #[tokio::test]
    async fn ai_escalation_clears_a_stale_partially_invalid_with_an_accurate_reason() {
        // Reconcile transition: the route starts mixed (PartiallyInvalid=True),
        // then the operator deletes the surviving rule. The stale True must be
        // cleared, and not with the generic "all rules are valid" wording — that
        // would contradict the Accepted=False now sitting beside it.
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx_with_raw_ai_backend("default", "chat");

        let mut route = route_with_bad_ai_rule(true);
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(
            route.status.as_ref().expect("status").parents[0]
                .conditions
                .iter()
                .find(|c| c.type_ == "PartiallyInvalid")
                .expect("PartiallyInvalid set on the mixed route")
                .status,
            "True",
            "precondition: the mixed route must be PartiallyInvalid"
        );

        // Drop the surviving rule, keeping the status from the previous reconcile.
        route.spec.rules.as_mut().expect("rules").truncate(1);
        handler.update_status(&mut route, &ctx, &[]);

        let conds = &route.status.as_ref().expect("status").parents[0].conditions;
        let partial = conds
            .iter()
            .find(|c| c.type_ == "PartiallyInvalid")
            .expect("stale PartiallyInvalid must still be present, cleared to False");
        assert_eq!(partial.status, "False");
        assert_eq!(partial.reason, "UnsupportedValue");
        assert_eq!(partial.message, "Route not accepted; no rule is active");

        let accepted = conds.iter().find(|c| c.type_ == "Accepted").expect("Accepted set");
        assert!(
            accepted.message.contains("retry.attempts"),
            "the escalation must carry the AI reason; got: {accepted:?}"
        );
    }

    fn ai_rule(backends: serde_json::Value, filters: serde_json::Value) -> HTTPRouteRule {
        serde_json::from_value(serde_json::json!({
            "backendRefs": backends,
            "filters": filters,
        }))
        .expect("AI rule must parse")
    }

    fn active_ai_proxies(active_count: usize) -> AiProxyAdmissionState {
        AiProxyAdmissionState {
            active_count,
            policy_blocked: false,
            config: None,
        }
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
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models"
            }]),
            extension_ref.clone(),
        );
        assert_eq!(
            ai_rule_invalid_reason(&pure_ai, "parent", |namespace, name| {
                assert_eq!((namespace, name), ("child", "ai"));
                Some(active_ai_proxies(1))
            }),
            None
        );

        let missing = ai_rule_invalid_reason(&pure_ai, "parent", |_, _| Some(active_ai_proxies(0))).unwrap();
        assert!(missing.contains("exactly one enabled AiProxy"), "{missing}");

        let mixed = ai_rule(
            serde_json::json!([
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "models"},
                {"name": "ordinary", "port": 80}
            ]),
            extension_ref,
        );
        let reason = ai_rule_invalid_reason(&mixed, "parent", |_, _| Some(active_ai_proxies(1))).unwrap();
        assert!(reason.contains("must not be mixed"), "{reason}");
    }

    #[test]
    fn zero_weight_refs_do_not_participate_in_ai_homogeneity() {
        let filters = serde_json::json!([{
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
            }
        }]);
        let rule = ai_rule(
            serde_json::json!([
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 1},
                {"name": "ordinary", "port": 80, "weight": 0}
            ]),
            filters,
        );
        assert!(ai_rule_invalid_reason(&rule, "parent", |_, _| Some(active_ai_proxies(1))).is_none());

        let no_positive_backend = ai_rule(
            serde_json::json!([
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0},
                {"name": "ordinary", "port": 80, "weight": 1}
            ]),
            serde_json::json!([{
                "type": "ExtensionRef",
                "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
                }
            }]),
        );
        assert!(
            ai_rule_invalid_reason(&no_positive_backend, "parent", |_, _| { Some(active_ai_proxies(1)) }).is_some()
        );
    }

    #[test]
    fn model_route_positive_override_promotes_zero_weight_slot_for_admission() {
        let rule = ai_rule(
            serde_json::json!([
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0},
                {"name": "ordinary", "port": 80, "weight": 0}
            ]),
            serde_json::json!([{
                "type": "ExtensionRef",
                "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"}
            }]),
        );
        let config: AiProxyConfig = serde_yaml::from_str(
            r#"
modelRoutes:
  - match: {model: virtual}
    action:
      candidates:
        - backend:
            group: edgion.io
            kind: EdgionBackend
            namespace: parent
            name: models
            occurrence: 0
          weight: 5
"#,
        )
        .unwrap();
        assert!(
            ai_rule_invalid_reason(&rule, "parent", |_, _| Some(AiProxyAdmissionState {
                active_count: 1,
                policy_blocked: false,
                config: Some(config.clone()),
            }))
            .is_none()
        );
    }

    #[tokio::test]
    async fn positive_weight_backend_requires_ai_capability() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_backend::EdgionBackendSpec;
        use kube::api::ObjectMeta;

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert(
            "apps/models".to_string(),
            Arc::new(EdgionBackend {
                metadata: ObjectMeta {
                    namespace: Some("apps".to_string()),
                    name: Some("models".to_string()),
                    ..Default::default()
                },
                spec: EdgionBackendSpec {
                    ai: None,
                    a2a: None,
                    mcp: None,
                    resolved_a2a: None,
                    resolved_mcp: None,
                    reconcile_outcome: None,
                },
                status: None,
            }),
        );
        processor.set_raw_store(raw);
        runtime.processor_registry.register(processor);

        let positive = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 1
            }]),
            serde_json::json!([]),
        );
        assert!(backend_variant_invalid_reason(&positive, "apps", &runtime).is_some());

        let zero = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0
            }]),
            serde_json::json!([]),
        );
        assert!(backend_variant_invalid_reason(&zero, "apps", &runtime).is_none());
    }

    #[tokio::test]
    async fn ordinary_rule_uses_raw_a2a_generation_not_processed_ai_generation() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_backend::{A2aBackendSpec, A2aBackendTarget, EdgionBackendSpec};
        use kube::api::ObjectMeta;

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        // The current raw generation has already changed from AI to A2A, while
        // the serial worker has not saved that generation to ServerCache yet.
        // Admission must classify the direct identity from RawStore. A missing
        // resolvedA2a is intentional: target resolution is not capability.
        let raw_a2a = EdgionBackend {
            metadata: ObjectMeta {
                namespace: Some("apps".to_string()),
                name: Some("agent".to_string()),
                ..Default::default()
            },
            spec: EdgionBackendSpec {
                ai: None,
                a2a: Some(A2aBackendSpec {
                    target: A2aBackendTarget {
                        service_ref: None,
                        endpoint: Some("https://agent.example.test".to_string()),
                    },
                    allow_insecure_http: false,
                }),
                mcp: None,
                resolved_a2a: None,
                resolved_mcp: None,
                reconcile_outcome: None,
            },
            status: None,
        };
        let processed_ai: EdgionBackend = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"namespace": "apps", "name": "agent"},
            "spec": {"ai": {
                "provider": "OpenAI",
                "credentialPool": {"credentials": [{
                    "name": "primary", "secretRef": {"name": "provider-key"}
                }]},
                "models": [{"name": "model-a"}]
            }}
        }))
        .unwrap();
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert("apps/agent".to_string(), Arc::new(raw_a2a));
        processor.set_raw_store(raw);
        processor.save(processed_ai, None);
        runtime.processor_registry.register(processor);

        let rule = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "agent", "weight": 1
            }]),
            serde_json::json!([]),
        );
        assert!(protocol_rule_invalid_reason_with_runtime(&rule, "apps", &runtime, false).is_none());
    }

    #[tokio::test]
    async fn model_route_validates_selected_zero_weight_backend_variant() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_backend::EdgionBackendSpec;
        use kube::api::ObjectMeta;

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        raw.upsert(
            "apps/models".to_string(),
            Arc::new(EdgionBackend {
                metadata: ObjectMeta {
                    namespace: Some("apps".to_string()),
                    name: Some("models".to_string()),
                    ..Default::default()
                },
                spec: EdgionBackendSpec {
                    ai: None,
                    a2a: None,
                    mcp: None,
                    resolved_a2a: None,
                    resolved_mcp: None,
                    reconcile_outcome: None,
                },
                status: None,
            }),
        );
        processor.set_raw_store(raw);
        runtime.processor_registry.register(processor);

        let candidate_config = |group: &str, kind: &str, name: &str, occurrence: u32| {
            serde_json::from_value::<AiProxyConfig>(serde_json::json!({
                "modelRoutes": [{
                    "match": {"model": "virtual"},
                    "action": {"candidates": [{"backend": {
                        "group": group,
                        "kind": kind,
                        "namespace": "apps",
                        "name": name,
                        "occurrence": occurrence
                    }, "weight": 0}]}
                }]
            }))
            .unwrap()
        };

        let missing_rule = ai_rule(
            serde_json::json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]),
            serde_json::json!([]),
        );
        let reason = model_route_context_invalid_reason_for_config(
            &missing_rule,
            "apps",
            &runtime,
            &candidate_config("edgion.io", "EdgionBackend", "missing", 0),
        )
        .unwrap();
        assert!(reason.contains("must resolve to one backendRef slot"), "{reason}");

        for (label, backend, config) in [
            (
                "denied",
                serde_json::json!({
                    "group": "edgion.io", "kind": "EdgionBackend", "name": "models",
                    "refDenied": {"targetNamespace": "apps", "targetName": "models"}
                }),
                candidate_config("edgion.io", "EdgionBackend", "models", 0),
            ),
            (
                "port",
                serde_json::json!({
                    "group": "edgion.io", "kind": "EdgionBackend", "name": "models", "port": 443
                }),
                candidate_config("edgion.io", "EdgionBackend", "models", 0),
            ),
            (
                "Service",
                serde_json::json!({"kind": "Service", "name": "models", "port": 80}),
                candidate_config("", "Service", "models", 0),
            ),
            (
                "MCP",
                serde_json::json!({"group": "edgion.io", "kind": "MCPBackend", "name": "models"}),
                candidate_config("edgion.io", "MCPBackend", "models", 0),
            ),
        ] {
            let rule = ai_rule(serde_json::json!([backend]), serde_json::json!([]));
            let reason = model_route_context_invalid_reason_for_config(&rule, "apps", &runtime, &config).unwrap();
            assert!(
                reason.contains("denied")
                    || reason.contains("must select an EdgionBackend")
                    || reason.contains("must resolve to one backendRef slot"),
                "{label}: {reason}"
            );
        }

        let rule = ai_rule(
            serde_json::json!([
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 1},
                {"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0}
            ]),
            serde_json::json!([]),
        );
        let config = candidate_config("edgion.io", "EdgionBackend", "models", 1);
        let reason = model_route_context_invalid_reason_for_config(&rule, "apps", &runtime, &config).unwrap();
        assert!(reason.contains("without spec.ai"), "{reason}");
    }

    #[tokio::test]
    async fn mcp_shape_uses_current_raw_backend_variant() {
        let runtime = runtime_with_a2a_plugin(&[
            (
                "tools",
                serde_json::json!({
                    "mcp": {
                        "target": {"endpoint": "https://tools.example.test/mcp"},
                        "transport": {"type": "StreamableHTTP"}
                    }
                }),
            ),
            (
                "agent",
                serde_json::json!({
                    "a2a": {"target": {"endpoint": "https://agent.example.test"}}
                }),
            ),
            (
                "tools-ready",
                serde_json::json!({
                    "mcp": {
                        "target": {"endpoint": "https://tools.example.test/mcp"},
                        "transport": {"type": "StreamableHTTP"}
                    },
                    "resolvedMcp": {
                        "target": {"type": "external", "normalized_endpoint": "https://tools.example.test/mcp"}
                    }
                }),
            ),
        ]);
        let rule = |name: &str, path_type: &str, path: &str| {
            serde_json::from_value::<HTTPRouteRule>(serde_json::json!({
                "matches": [{"path": {"type": path_type, "value": path}}],
                "backendRefs": [{
                    "group": "edgion.io", "kind": "EdgionBackend", "name": name
                }]
            }))
            .expect("MCP route fixture must deserialize")
        };

        assert!(runtime_mcp_rule_invalid_reason(&rule("tools", "Exact", "/mcp"), "apps", &runtime, false).is_none());
        assert!(
            runtime_mcp_rule_invalid_reason(&rule("tools-ready", "Exact", "/mcp"), "apps", &runtime, false).is_none(),
            "resolvedMcp readiness must not change route-shape intent"
        );
        assert!(
            runtime_mcp_rule_invalid_reason(&rule("tools", "PathPrefix", "/mcp"), "apps", &runtime, false)
                .is_some_and(|reason| reason.contains("path type Exact"))
        );
        let invalid = rule("tools", "PathPrefix", "/mcp");
        assert!(protocol_rule_invalid_reason_after_policy(&invalid, "apps", &runtime, false, true).is_none());
        assert!(protocol_rule_invalid_reason_after_policy(&invalid, "apps", &runtime, false, false).is_some());
        assert!(protocol_rule_invalid_reason_after_policy(&invalid, "apps", &runtime, false, true).is_none());
        assert!(
            runtime_mcp_rule_invalid_reason(&rule("tools", "Exact", "/mcp"), "apps", &runtime, true)
                .is_some_and(|reason| reason.contains("delegation"))
        );

        // Ordinary A2A or missing targets are not implicit MCP evidence.
        assert!(runtime_mcp_rule_invalid_reason(&rule("agent", "PathPrefix", "/"), "apps", &runtime, false).is_none());
        assert!(
            runtime_mcp_rule_invalid_reason(&rule("missing", "PathPrefix", "/"), "apps", &runtime, false).is_none()
        );

        let mut denied = rule("tools", "PathPrefix", "/");
        denied.backend_refs.as_mut().unwrap()[0].ref_denied = Some(RefDenied {
            target_namespace: "apps".to_string(),
            target_name: "tools".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        });
        assert!(runtime_mcp_rule_invalid_reason(&denied, "apps", &runtime, false).is_none());
    }

    #[tokio::test]
    async fn mcp_raw_variant_delete_recreate_and_readiness_transitions_are_current() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let raw = Arc::new(LocalRawStore::new());
        processor.set_raw_store(raw.clone());
        runtime.processor_registry.register(processor);
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/mcp"}}],
            "backendRefs": [{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "target"
            }]
        }))
        .expect("MCP transition route fixture must deserialize");
        let backend = |spec: serde_json::Value| {
            serde_json::from_value::<EdgionBackend>(serde_json::json!({
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionBackend",
                "metadata": {"namespace": "apps", "name": "target"},
                "spec": spec
            }))
            .expect("backend transition fixture must deserialize")
        };
        let mcp = || {
            backend(serde_json::json!({
                "mcp": {
                    "target": {"endpoint": "https://tools.example.test/mcp"},
                    "transport": {"type": "StreamableHTTP"}
                }
            }))
        };

        assert!(runtime_mcp_rule_invalid_reason(&rule, "apps", &runtime, false).is_none());
        raw.upsert("apps/target".to_string(), Arc::new(mcp()));
        assert!(runtime_mcp_rule_invalid_reason(&rule, "apps", &runtime, false).is_some());

        raw.upsert(
            "apps/target".to_string(),
            Arc::new(backend(serde_json::json!({
                "a2a": {"target": {"endpoint": "https://agent.example.test"}}
            }))),
        );
        assert!(runtime_mcp_rule_invalid_reason(&rule, "apps", &runtime, false).is_none());

        let mut ready = mcp();
        ready.spec.resolved_mcp = Some(edgion_resources::resources::edgion_backend::ResolvedMcpBackend {
            target: edgion_resources::resources::edgion_backend::ResolvedMcpTarget::External {
                normalized_endpoint: "https://tools.example.test/mcp".to_string(),
            },
        });
        raw.upsert("apps/target".to_string(), Arc::new(ready));
        assert!(runtime_mcp_rule_invalid_reason(&rule, "apps", &runtime, false).is_some());

        raw.remove("apps/target");
        assert!(runtime_mcp_rule_invalid_reason(&rule, "apps", &runtime, false).is_none());
        raw.upsert("apps/target".to_string(), Arc::new(mcp()));
        assert!(runtime_mcp_rule_invalid_reason(&rule, "apps", &runtime, false).is_some());
    }

    #[tokio::test]
    async fn invalid_mcp_rule_uses_existing_partial_and_route_rejection_status() {
        let runtime = runtime_with_a2a_plugin(&[(
            "tools",
            serde_json::json!({
                "mcp": {
                    "target": {"endpoint": "https://tools.example.test/mcp"},
                    "transport": {"type": "StreamableHTTP"}
                }
            }),
        )]);
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime);
        let handler = HttpRouteHandler::default();
        let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"namespace": "apps", "name": "mcp"},
            "spec": {
                "parentRefs": [{"name": "gateway"}],
                "rules": [
                    {
                        "matches": [{"path": {"type": "PathPrefix", "value": "/mcp"}}],
                        "backendRefs": [{
                            "group": "edgion.io", "kind": "EdgionBackend", "name": "tools"
                        }]
                    },
                    {
                        "matches": [{"path": {"type": "Exact", "value": "/health"}}],
                        "backendRefs": [{"name": "service", "port": 8080}]
                    }
                ]
            }
        }))
        .expect("HTTPRoute fixture must deserialize");

        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == "PartiallyInvalid" && condition.status == "True"));

        route.spec.rules.as_mut().unwrap().truncate(1);
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == "Accepted"
                && condition.status == "False"
                && condition.message.contains("path type Exact")
        }));
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == "PartiallyInvalid" && condition.status == "False"));
    }

    #[tokio::test]
    async fn mcp_cross_namespace_grant_allowed_and_revoked_recomputes_route() {
        use edgion_resources::resources::reference_grant::{
            ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo,
        };
        use kube::api::ObjectMeta;

        let runtime = runtime_with_a2a_plugin(&[(
            "tools",
            serde_json::json!({
                "mcp": {
                    "target": {"endpoint": "https://tools.example.test/mcp"},
                    "transport": {"type": "StreamableHTTP"}
                }
            }),
        )]);
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());
        let route = || {
            serde_json::from_value::<HTTPRoute>(serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "HTTPRoute",
                "metadata": {"namespace": "infra", "name": "mcp"},
                "spec": {
                    "parentRefs": [{"name": "gateway"}],
                    "rules": [{
                        "matches": [{"path": {"type": "PathPrefix", "value": "/mcp"}}],
                        "backendRefs": [{
                            "group": "edgion.io", "kind": "EdgionBackend",
                            "namespace": "apps", "name": "tools"
                        }]
                    }]
                }
            }))
            .expect("cross-namespace MCP route fixture must deserialize")
        };
        let handler = HttpRouteHandler::default();

        let ProcessResult::Continue(denied) = handler.parse(route(), &ctx).await else {
            panic!("denied route parse must continue");
        };
        assert!(denied.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0]
            .ref_denied
            .is_some());
        assert!(
            runtime_mcp_rule_invalid_reason(&denied.spec.rules.as_ref().unwrap()[0], "infra", &runtime, false)
                .is_none()
        );

        let grant = ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some("apps".to_string()),
                name: Some("allow-infra-mcp".to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: "gateway.networking.k8s.io".to_string(),
                    kind: "HTTPRoute".to_string(),
                    namespace: "infra".to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: "edgion.io".to_string(),
                    kind: "EdgionBackend".to_string(),
                    name: Some("tools".to_string()),
                }],
            },
        };
        runtime
            .ref_grant_store
            .upsert("apps/allow-infra-mcp".to_string(), Arc::new(grant));
        let ProcessResult::Continue(mut allowed) = handler.parse(route(), &ctx).await else {
            panic!("allowed route parse must continue");
        };
        assert!(
            allowed.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0]
                .ref_denied
                .is_none()
        );
        assert!(
            runtime_mcp_rule_invalid_reason(&allowed.spec.rules.as_ref().unwrap()[0], "infra", &runtime, false)
                .is_some_and(|reason| reason.contains("path type Exact"))
        );
        HttpRouteHandler::default().update_status(&mut allowed, &ctx, &[]);
        assert_eq!(allowed.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));

        runtime.ref_grant_store.remove("apps/allow-infra-mcp");
        let ProcessResult::Continue(revoked) = handler.parse(route(), &ctx).await else {
            panic!("revoked route parse must continue");
        };
        assert!(
            revoked.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0]
                .ref_denied
                .is_some()
        );
        assert!(
            runtime_mcp_rule_invalid_reason(&revoked.spec.rules.as_ref().unwrap()[0], "infra", &runtime, false)
                .is_none()
        );
    }

    #[tokio::test]
    async fn ai_proxy_admission_uses_policy_stamped_cache_and_preserves_block_gate() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::raw_store::LocalRawStore;
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use edgion_resources::resources::edgion_backend::EdgionBackendSpec;
        use edgion_resources::resources::edgion_plugins::{
            AiProxyConfig, EdgionPluginsSpec, MockConfig, RequestFilterEntry, UpstreamResponseEntry,
        };
        use edgion_resources::resources::plugin_policy::PluginPolicyAction;
        use kube::api::ObjectMeta;

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::default()),
            runtime.clone(),
        ));

        let mut raw_plugins = EdgionPlugins::new(
            "ai",
            EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::AiProxy(
                    serde_json::from_value::<AiProxyConfig>(serde_json::json!({}))
                        .expect("empty AiProxy config must use stable defaults"),
                ))]),
                ..Default::default()
            },
        );
        raw_plugins.metadata.namespace = Some("apps".to_string());

        let mut bypassed_plugins = raw_plugins.clone();
        bypassed_plugins
            .spec
            .request_plugins
            .as_mut()
            .expect("requestPlugins set by fixture")[0]
            .policy_action = Some(PluginPolicyAction::Bypass);
        let mut blocked_ai_plugins = raw_plugins.clone();
        blocked_ai_plugins
            .spec
            .request_plugins
            .as_mut()
            .expect("requestPlugins set by fixture")[0]
            .policy_action = Some(PluginPolicyAction::Block { status: 451 });
        let mut blocked_response = UpstreamResponseEntry::new(EdgionPlugin::Mock(MockConfig::default()));
        blocked_response.policy_action = Some(PluginPolicyAction::Block { status: 452 });
        let mut blocked_other_plugin = bypassed_plugins.clone();
        blocked_other_plugin.spec.upstream_response_plugins = Some(vec![blocked_response]);

        let raw_store = Arc::new(LocalRawStore::new());
        raw_store.upsert("apps/ai".to_string(), Arc::new(raw_plugins));
        processor.set_raw_store(raw_store);
        processor.save(bypassed_plugins, None);
        runtime.processor_registry.register(processor.clone());

        assert_eq!(
            ai_proxy_admission_state(&runtime, "apps", "ai"),
            Some(AiProxyAdmissionState::default()),
            "Bypass in the processed cache must suppress the active raw entry"
        );

        let rule = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models"
            }]),
            serde_json::json!([{
                "type": "ExtensionRef",
                "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "ai"
                }
            }]),
        );
        assert!(
            ai_rule_invalid_reason(&rule, "apps", |namespace, name| {
                ai_proxy_admission_state(&runtime, namespace, name)
            })
            .is_some(),
            "Bypass must leave the AI rule without an active AiProxy"
        );

        processor.save(blocked_ai_plugins, None);
        assert_eq!(
            ai_proxy_admission_state(&runtime, "apps", "ai"),
            Some(AiProxyAdmissionState {
                active_count: 0,
                policy_blocked: true,
                config: None,
            }),
            "Block must remain distinct from Bypass"
        );
        assert!(
            ai_rule_invalid_reason(&rule, "apps", |namespace, name| {
                ai_proxy_admission_state(&runtime, namespace, name)
            })
            .is_none(),
            "Block must preserve the rule so the Gateway compiles PluginPolicyBlock"
        );

        let backend_processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let backend_raw_store = Arc::new(LocalRawStore::new());
        backend_raw_store.upsert(
            "apps/models".to_string(),
            Arc::new(EdgionBackend {
                metadata: ObjectMeta {
                    namespace: Some("apps".to_string()),
                    name: Some("models".to_string()),
                    ..Default::default()
                },
                spec: EdgionBackendSpec {
                    ai: None,
                    a2a: None,
                    mcp: None,
                    resolved_a2a: None,
                    resolved_mcp: None,
                    reconcile_outcome: None,
                },
                status: None,
            }),
        );
        backend_processor.set_raw_store(backend_raw_store);
        runtime.processor_registry.register(backend_processor);
        assert!(
            protocol_rule_invalid_reason_with_runtime(&rule, "apps", &runtime, false).is_some(),
            "a stale processed Block must not override the current raw, canonically stamped plugin generation"
        );

        processor.save(blocked_other_plugin, None);
        assert_eq!(
            ai_proxy_admission_state(&runtime, "apps", "ai"),
            Some(AiProxyAdmissionState {
                active_count: 0,
                policy_blocked: true,
                config: None,
            }),
            "a Block on another stage/plugin must preserve the chain even when AiProxy is Bypass"
        );
    }

    #[test]
    fn plugin_policy_block_precedes_every_ai_admission_drop() {
        let extension_ref = serde_json::json!([{
            "type": "ExtensionRef",
            "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "blocked"
            }
        }]);
        let blocked = AiProxyAdmissionState {
            active_count: 0,
            policy_blocked: true,
            config: None,
        };
        let cases = [
            (
                "mixed backends",
                ai_rule(
                    serde_json::json!([
                        {"group": "edgion.io", "kind": "EdgionBackend", "name": "models"},
                        {"name": "ordinary", "port": 80}
                    ]),
                    extension_ref.clone(),
                ),
            ),
            (
                "AI backend port",
                ai_rule(
                    serde_json::json!([{
                        "group": "edgion.io", "kind": "EdgionBackend", "name": "models", "port": 443
                    }]),
                    extension_ref.clone(),
                ),
            ),
            (
                "AI backend filter",
                ai_rule(
                    serde_json::json!([{
                        "group": "edgion.io", "kind": "EdgionBackend", "name": "models",
                        "filters": [{"type": "URLRewrite", "urlRewrite": {
                            "path": {"type": "ReplaceFullPath", "replaceFullPath": "/v2"}
                        }}]
                    }]),
                    extension_ref.clone(),
                ),
            ),
            (
                "retry cap",
                serde_json::from_value(serde_json::json!({
                    "backendRefs": [{
                        "group": "edgion.io", "kind": "EdgionBackend", "name": "models"
                    }],
                    "filters": extension_ref,
                    "retry": {"attempts": 8}
                }))
                .expect("blocked retry fixture must parse"),
            ),
        ];
        for (case, rule) in cases {
            assert!(
                ai_rule_invalid_reason(&rule, "apps", |_, _| Some(blocked.clone())).is_none(),
                "PluginPolicyBlock must preserve the rule before {case} can drop its gate"
            );
        }

        let block_and_missing = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models"
            }]),
            serde_json::json!([
                {"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "blocked"
                }},
                {"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "missing"
                }}
            ]),
        );
        assert!(
            ai_rule_invalid_reason(&block_and_missing, "apps", |_, name| {
                (name == "blocked").then(|| blocked.clone())
            })
            .is_none(),
            "a sibling missing ExtensionRef must not remove an unconditional policy block gate"
        );
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
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models",
                "filters": [
                    {"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-tenant", "value": "a"}]}},
                    {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"set": [{"name": "x-ai", "value": "yes"}]}}
                ]
            }]),
            extension_ref.clone(),
        );
        assert!(ai_rule_invalid_reason(&allowed, "default", |_, _| Some(active_ai_proxies(1))).is_none());

        let rejected = ai_rule(
            serde_json::json!([{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models",
                "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {"type": "ReplaceFullPath", "replaceFullPath": "/v2"}}}]
            }]),
            extension_ref,
        );
        let reason = ai_rule_invalid_reason(&rejected, "default", |_, _| Some(active_ai_proxies(1))).unwrap();
        assert!(reason.contains("URLRewrite"), "{reason}");
    }

    #[test]
    fn ai_rule_rejects_more_than_seven_explicit_retries() {
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
            "backendRefs": [{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "models"
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

        let reason = ai_rule_invalid_reason(&rule, "default", |_, _| Some(active_ai_proxies(1))).unwrap();
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
                "rules": [{
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": {
                            "group": "edgion.io", "kind": "EdgionPlugins",
                            "name": "parent-plugins"
                        }
                    }]
                }],
                "resolvedRules": [{
                    "backendRefs": [{
                        "group": "edgion.io", "kind": "EdgionBackend",
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
            Some("EdgionBackend".to_string()),
            Some("child".to_string()),
            "models-a".to_string(),
        )];
        let first = HttpRouteHandler::register_backend_refs(&route, &first_tuples, &ctx);
        assert_eq!(
            first.affected,
            vec!["edgion.io/EdgionBackend/child/models-a".to_string()]
        );
        HttpRouteHandler::register_plugin_refs(&route, &ctx);
        let mut dependencies = ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key());
        dependencies.sort();
        assert_eq!(
            dependencies,
            vec!["child/ai-plugins".to_string(), "infra/parent-plugins".to_string()]
        );

        let resolved = route.spec.resolved_rules.as_mut().unwrap();
        resolved[0].backend_refs.as_mut().unwrap()[0].name = "models-b".to_string();
        resolved[0].filters = None;
        let second_tuples = vec![(
            Some("edgion.io".to_string()),
            Some("EdgionBackend".to_string()),
            Some("child".to_string()),
            "models-b".to_string(),
        )];
        let second = HttpRouteHandler::register_backend_refs(&route, &second_tuples, &ctx);
        assert_eq!(
            second.dropped,
            vec!["edgion.io/EdgionBackend/child/models-a".to_string()]
        );
        assert_eq!(second.affected.len(), 2);
        HttpRouteHandler::register_plugin_refs(&route, &ctx);
        assert_eq!(
            ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key()),
            vec!["infra/parent-plugins".to_string()],
            "operator-declared refs must survive removal from effective rules"
        );
    }

    #[test]
    fn flattened_backend_refs_remain_registered_after_rule_withdrawal() {
        let ctx = make_ctx();
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "parent", "namespace": "infra"},
            "spec": {"resolvedRules": []}
        }))
        .unwrap();
        let flattened_tuples = vec![(
            Some("edgion.io".to_string()),
            Some("EdgionBackend".to_string()),
            Some("child".to_string()),
            "missing-or-denied".to_string(),
        )];

        let delta = HttpRouteHandler::register_backend_refs(&route, &flattened_tuples, &ctx);

        assert_eq!(
            delta.affected,
            vec!["edgion.io/EdgionBackend/child/missing-or-denied".to_string()]
        );
        assert_eq!(
            ctx.runtime()
                .backend_ref_mgr
                .get_dependencies(&HttpRouteHandler::create_resource_ref(&route).key()),
            delta.affected
        );
    }

    #[tokio::test]
    async fn delegated_mcp_withdrawal_retains_dependencies_and_requeues_parent() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::{EdgionBackendHandler, EdgionPluginsHandler};
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_plugins::EdgionPluginsSpec;
        use std::sync::atomic::Ordering;

        let runtime = runtime_with_a2a_plugin(&[(
            "tools",
            serde_json::json!({
                "mcp": {
                    "target": {"endpoint": "https://tools.example.test/mcp"},
                    "transport": {"type": "StreamableHTTP"}
                }
            }),
        )]);
        let route_processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
            "HTTPRoute",
            32,
            Arc::new(HttpRouteHandler::default()),
            runtime.clone(),
        ));
        let child: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"namespace": "apps", "name": "child"},
            "spec": {
                "parentRefs": [{
                    "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                    "namespace": "apps", "name": "parent"
                }],
                "rules": [
                    {
                        "matches": [{"path": {"type": "Exact", "value": "/api/mcp"}}],
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": {
                                "group": "edgion.io", "kind": "EdgionPlugins", "name": "child-plugins"
                            }
                        }],
                        "backendRefs": [{
                            "group": "edgion.io", "kind": "EdgionBackend", "name": "tools"
                        }]
                    },
                    {
                        "matches": [{"path": {"type": "Exact", "value": "/api/health"}}],
                        "backendRefs": [{"name": "health", "port": 8080}]
                    }
                ]
            }
        }))
        .expect("child route fixture must deserialize");
        let child_raw = Arc::new(LocalRawStore::new());
        child_raw.upsert("apps/child".to_string(), Arc::new(child.clone()));
        route_processor.set_raw_store(child_raw);
        route_processor.save(child, None);
        runtime.processor_registry.register(route_processor.clone());

        let parent: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"namespace": "apps", "name": "parent"},
            "spec": {
                "parentRefs": [{"name": "gateway"}],
                "rules": [
                    {
                        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": {
                                "group": "edgion.io", "kind": "EdgionPlugins", "name": "parent-plugins"
                            }
                        }],
                        "backendRefs": [{
                            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                            "namespace": "apps", "name": "child"
                        }]
                    },
                    {
                        "matches": [{"path": {"type": "Exact", "value": "/direct-mcp"}}],
                        "backendRefs": [{
                            "group": "edgion.io", "kind": "EdgionBackend", "name": "tools"
                        }]
                    }
                ]
            }
        }))
        .expect("parent route fixture must deserialize");
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());
        let ProcessResult::Continue(mut parsed) = HttpRouteHandler::default().parse(parent, &ctx).await else {
            panic!("parent parse must continue");
        };

        let resolved = parsed.spec.resolved_rules.as_ref().expect("delegation must flatten");
        assert_eq!(resolved.len(), 1, "only the ordinary child sibling should survive");
        assert_eq!(
            resolved[0].matches.as_ref().unwrap()[0]
                .path
                .as_ref()
                .unwrap()
                .value
                .as_deref(),
            Some("/api/health")
        );
        assert!(parsed.spec.delegation_issues.iter().any(|issue| {
            matches!(issue, DelegationIssue::RuleDropped { message } if message.contains("MCP rules do not support HTTPRoute delegation"))
        }));
        let parent_ref = HttpRouteHandler::create_resource_ref(&parsed);
        let mut plugin_dependencies = runtime.plugin_ref_mgr.get_dependencies(&parent_ref.key());
        plugin_dependencies.sort();
        assert_eq!(
            plugin_dependencies,
            vec!["apps/child-plugins".to_string(), "apps/parent-plugins".to_string()]
        );
        assert_eq!(
            runtime.backend_ref_mgr.get_dependencies(&parent_ref.key()),
            vec!["edgion.io/EdgionBackend/apps/tools".to_string()]
        );

        HttpRouteHandler::default().update_status(&mut parsed, &ctx, &[]);
        let conditions = &parsed.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == "PartiallyInvalid" && condition.status == "True"));
        assert!(!conditions.iter().any(|condition| {
            condition.type_ == "Accepted"
                && condition.status == "False"
                && condition.reason == "UnsupportedValue"
                && condition.message.contains("MCP")
        }));

        // Parsing registers delegation and schedules one child-route refresh.
        // Drain that unrelated item before observing dependency-triggered
        // parent requeues below.
        tokio::time::timeout(std::time::Duration::from_secs(1), route_processor.workqueue().dequeue())
            .await
            .expect("parse-triggered route requeue should become ready")
            .expect("HTTPRoute workqueue should stay open");

        let plugin_handler = EdgionPluginsHandler::default();
        for (namespace, name) in [("apps", "child-plugins"), ("apps", "parent-plugins")] {
            let before = route_processor
                .workqueue()
                .metrics()
                .delayed_total
                .load(Ordering::Relaxed);
            let mut plugins = EdgionPlugins::new(name, EdgionPluginsSpec::default());
            plugins.metadata.namespace = Some(namespace.to_string());
            plugin_handler.on_change(&plugins, &ctx).await;
            assert_eq!(
                route_processor
                    .workqueue()
                    .metrics()
                    .delayed_total
                    .load(Ordering::Relaxed),
                before + 1,
                "plugin change must schedule the parent route"
            );
            tokio::time::timeout(std::time::Duration::from_secs(1), route_processor.workqueue().dequeue())
                .await
                .expect("plugin-triggered parent requeue should become ready")
                .expect("HTTPRoute workqueue should stay open");
        }

        let backend: EdgionBackend = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"namespace": "apps", "name": "tools"},
            "spec": {
                "mcp": {
                    "target": {"endpoint": "https://tools.example.test/mcp"},
                    "transport": {"type": "StreamableHTTP"}
                }
            }
        }))
        .expect("MCP backend fixture must deserialize");
        let backend_handler = EdgionBackendHandler::new();
        for event in ["change", "delete", "recreate"] {
            let before = route_processor
                .workqueue()
                .metrics()
                .delayed_total
                .load(Ordering::Relaxed);
            match event {
                "delete" => backend_handler.on_delete(&backend, &ctx).await,
                _ => backend_handler.on_change(&backend, &ctx).await,
            }
            assert_eq!(
                route_processor
                    .workqueue()
                    .metrics()
                    .delayed_total
                    .load(Ordering::Relaxed),
                before + 1,
                "backend {event} must schedule the withdrawn parent route"
            );
            tokio::time::timeout(std::time::Duration::from_secs(1), route_processor.workqueue().dequeue())
                .await
                .expect("backend-triggered parent requeue should become ready")
                .expect("HTTPRoute workqueue should stay open");
        }
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

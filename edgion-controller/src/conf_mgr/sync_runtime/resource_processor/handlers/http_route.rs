//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use std::collections::HashSet;
use std::sync::Arc;

use super::super::ref_grant::{
    is_cross_ns_ref_allowed, read_after_registering_cross_ns_dependency, validate_http_route_if_enabled,
};
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
#[cfg(test)]
use edgion_resources::resources::edgion_plugins::{AiProxyConfig, EdgionPlugin, EdgionPlugins};
use edgion_resources::resources::extension_ref::{
    is_supported_edgion_plugins_ref, validate_extension_ref_filter, ExtensionRefValidationError,
};
use edgion_resources::resources::http_route::{
    DelegationIssue, HTTPBackendRefKind, HTTPRouteFilterType, HTTPRouteRule, HTTPRouteStatus, GATEWAY_API_GROUP,
};
#[cfg(test)]
use edgion_resources::resources::http_route_ai_admission::{self as shared_ai_admission, AiProxyAdmission};
use edgion_resources::resources::http_route_ai_admission::{ResolvedAiAdmission, ResolvedForwardingTarget};
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
                "HTTPRoute",
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
        extra_tuples: &[super::route_utils::OwnedBackendRefTuple],
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

        let dropped = super::route_utils::register_service_backend_refs(
            ResourceKind::HTTPRoute,
            route_ns,
            route_name,
            &backend_refs_list,
            ctx.runtime(),
        );
        // Keep the owner identity identical to register_service_backend_refs()
        // and clear_service_backend_refs(). File input can omit namespace;
        // namespaced route semantics normalize that case to "default".
        let owner = ResourceRef::new(
            ResourceKind::HTTPRoute,
            Some(route_ns.to_string()),
            route_name.to_string(),
        );
        super::request_mirror::register_service_refs(
            &owner,
            route_ns,
            route
                .effective_rules()
                .into_iter()
                .flatten()
                .flat_map(super::request_mirror::refs),
            ctx.runtime(),
        );
        dropped
    }

    /// Record cross-namespace references from backend_refs.
    ///
    /// `extra_namespaces` carries the cross-namespace targets of flattened
    /// delegation backendRefs so ReferenceGrant changes in those namespaces
    /// requeue this parent route. Extend the set cleared at parse entry without
    /// removing dependencies registered before mirror authorization checks.
    fn record_cross_ns_refs(route: &HTTPRoute, extra_namespaces: &HashSet<String>, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().cross_ns_ref_mgr;

        // Collect cross-namespace references from rules
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                for mirror in super::request_mirror::refs(rule) {
                    if let Some(target_ns) = mirror.namespace.as_ref().filter(|ns| ns.as_str() != route_ns) {
                        manager.add_ref(target_ns.clone(), resource_ref.clone());
                    }
                }
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

    /// Register all EdgionPlugins ExtensionRefs for plugin-change requeue.
    ///
    /// BackendRef filters participate because a referenced generation that
    /// adds or removes WAF changes this route's ownership validity. Replacing
    /// the full ref set on every reconcile also handles delete-to-create
    /// coalescing.
    fn register_plugin_refs(route: &HTTPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        let manager = &ctx.runtime().plugin_ref_mgr;
        manager.clear_resource_refs(&resource_ref);

        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        // Keep dependencies declared on the operator route as well as those
        // inherited by flattened child rules. This runs before protocol
        // filtering so a withdrawn rule can still be restored when its
        // PluginPolicy changes again.
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
                    if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
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

    /// Replace this route's EdgionBackend dependency set and return the
    /// old/new union plus the removed subset. The union is needed because an
    /// EBTP attached to either side may change the route's Gateway ancestors.
    fn register_backend_refs(
        route: &HTTPRoute,
        flattened_tuples: &[super::route_utils::OwnedBackendRefTuple],
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
    fn finalize_transport(&self, route: &mut HTTPRoute, _ctx: &HandlerContext) {
        route.spec.resolved_status_controller = Some(self.controller_name.clone());
    }

    fn validate(&self, route: &HTTPRoute, _ctx: &HandlerContext) -> Vec<String> {
        let errors = validate_http_route_if_enabled(route);
        // Note: per-rule validation (filter conflicts, malformed CORS) is handled by
        // find_invalid_rule_details() in update_status(), which marks individual rules
        // as invalid via PartiallyInvalid condition rather than rejecting the entire route.
        errors
    }

    async fn parse(&self, mut route: HTTPRoute, _ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        let grant_owner = Self::create_resource_ref(&route);
        _ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&grant_owner);
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let route_ns = route_ns.as_str();

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

        // Mark denied cross-namespace references on the operator-authored rules.
        // MUST run BEFORE the delegation flatten: flatten clones non-delegating
        // sibling rules verbatim into resolved_rules, and the gateway compiles
        // resolved_rules INSTEAD of spec.rules — marking afterwards would leave
        // the compiled copies with ref_denied=None (ReferenceGrant bypass).
        if let Some(rules) = &mut route.spec.rules {
            for rule in rules {
                super::request_mirror::resolve_rule(rule, route_ns, |backend| {
                    read_after_registering_cross_ns_dependency(
                        _ctx.runtime(),
                        backend.namespace.as_deref().unwrap_or(route_ns),
                        &grant_owner,
                        || super::request_mirror::allowed(backend, route_ns, ResourceKind::HTTPRoute, _ctx.runtime()),
                    )
                });
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
                                let allowed = read_after_registering_cross_ns_dependency(
                                    _ctx.runtime(),
                                    backend_ns,
                                    &grant_owner,
                                    || {
                                        is_cross_ns_ref_allowed(
                                            route_ns,
                                            "HTTPRoute",
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

        // HTTPRoute delegation: expand delegation trees into resolved_rules.
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
        // Dependency registration below must inspect the rules that the
        // Gateway will actually compile, including delegated child filters.
        route.spec.resolved_rules = delegation_outcome.resolved_rules.clone();

        // Register before protocol filtering so a rule removed below retains
        // the dependency that can make it eligible again on a later reconcile.
        Self::register_plugin_refs(&route, _ctx);

        if let Some(rules) = &mut route.spec.resolved_rules {
            let mut resolved_index = 0usize;
            rules.retain(|rule| {
                let reason = backend_waf_invalid_reason(rule, route_ns, _ctx.runtime());
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

        // Register dependencies before reading backend source facts. Recompute
        // flattened slots after delegation; never trust a child's old sidecar.
        populate_ai_admission(&mut route, route_ns, _ctx.runtime());

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

        let tracker_changed =
            remove_from_attached_route_tracker(ResourceKind::HTTPRoute, route_ns, route_name, ctx.runtime());
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_status(&self, route: &mut HTTPRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        // Static per-rule findings drop only the offending rule through
        // `invalid_rule_indices` + PartiallyInvalid. They become
        // Accepted=False only when no effective rule remains.
        let ownership_accepted_errors = backend_waf_ownership_errors(route, route_ns, _ctx.runtime());

        // AI admission is Gateway-scoped. A source-only diagnostic neither
        // drops a rule nor activates a fallback, so it must not be represented
        // by Gateway API's PartiallyInvalid condition.

        let mut resolved_refs_errors = validate_http_backend_refs(route, route_ns, _ctx.runtime());
        resolved_refs_errors.extend(super::request_mirror::errors(
            route_ns,
            ResourceKind::HTTPRoute,
            route
                .effective_rules()
                .into_iter()
                .flatten()
                .flat_map(super::request_mirror::refs),
            _ctx.runtime(),
        ));
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
        // Any static rule failure or delegation drop rejects an empty attachment.
        // AI source diagnostics never withdraw a rule or participate here.
        let attachment_rejects_route =
            (!invalid_details.is_empty() || !delegation_dropped.is_empty()) && serves_no_rule;

        let hostname_error = route.spec.validate_hostnames().err();
        let mut validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);
        validation_accepted_errors.extend(ownership_accepted_errors.clone());
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
        let route_rejected =
            hostname_error.is_some() || attachment_rejects_route || !ownership_accepted_errors.is_empty();

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
            route.spec.parent_refs.as_ref().filter(|_| hostname_error.is_none()),
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg(test)]
struct AiProxyAdmissionState {
    active_count: usize,
    config: Option<AiProxyConfig>,
}

#[cfg(test)]
fn ai_proxy_admission_state(runtime: &ControllerRuntime, namespace: &str, name: &str) -> Option<AiProxyAdmissionState> {
    let processor = runtime.processor_registry.get("EdgionPlugins")?;
    let key = format!("{namespace}/{name}");
    let plugins = processor.get_cached_any(&key)?.downcast::<EdgionPlugins>().ok()?;
    let mut state = AiProxyAdmissionState::default();
    for entry in plugins
        .spec
        .request_plugins
        .iter()
        .flatten()
        .filter(|entry| entry.is_enabled())
    {
        if let EdgionPlugin::AiProxy(config) = &entry.plugin {
            state.active_count += 1;
            state.config = Some(config.clone());
        }
    }
    Some(state)
}

/// Resolve only declarative backend facts, never credentials or runtime objects.
fn resolve_ai_admission(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> ResolvedAiAdmission {
    let backend_processor = runtime.processor_registry.get("EdgionBackend");
    let backends = rule
        .backend_refs
        .iter()
        .flatten()
        .map(|backend| {
            let namespace = backend.namespace.as_deref().unwrap_or(route_namespace);
            match backend.backend_kind() {
                HTTPBackendRefKind::Service => {
                    if super::route_utils::service_exists(namespace, &backend.name, runtime) {
                        ResolvedForwardingTarget::Service
                    } else {
                        ResolvedForwardingTarget::Missing
                    }
                }
                HTTPBackendRefKind::EdgionBackend => {
                    let key = format!("{namespace}/{}", backend.name);
                    match backend_processor
                        .as_ref()
                        .and_then(|processor| processor.get_raw_any(&key))
                    {
                        Some(raw) => match raw.downcast::<EdgionBackend>() {
                            Ok(resource) => ResolvedForwardingTarget::EdgionBackend {
                                spec_valid: resource.spec.validate().is_ok(),
                                canonical_models: resource
                                    .spec
                                    .ai
                                    .models
                                    .iter()
                                    .map(|model| model.name.clone())
                                    .collect(),
                            },
                            Err(_) => ResolvedForwardingTarget::Incompatible,
                        },
                        None => ResolvedForwardingTarget::Missing,
                    }
                }
                HTTPBackendRefKind::RouteDelegation | HTTPBackendRefKind::Unsupported => {
                    ResolvedForwardingTarget::Missing
                }
            }
        })
        .collect();
    ResolvedAiAdmission { backends }
}

fn populate_ai_admission(route: &mut HTTPRoute, route_namespace: &str, runtime: &ControllerRuntime) {
    for rule in route
        .spec
        .rules
        .iter_mut()
        .flatten()
        .chain(route.spec.resolved_rules.iter_mut().flatten())
    {
        rule.resolved_ai_admission = Some(resolve_ai_admission(rule, route_namespace, runtime));
    }
}

#[cfg(test)]
fn source_ai_admission(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    mut lookup: impl FnMut(&str, &str) -> Option<AiProxyAdmissionState>,
) -> (usize, Vec<AiProxyConfig>, Option<String>) {
    let mut count = 0;
    let mut configs = Vec::new();
    let mut missing = None;
    for filter in rule
        .filters
        .iter()
        .flatten()
        .filter(|filter| filter.filter_type == HTTPRouteFilterType::ExtensionRef)
    {
        let Some(reference) = &filter.extension_ref else {
            continue;
        };
        if !is_supported_edgion_plugins_ref(reference) {
            continue;
        }
        let namespace = reference.resolved_namespace.as_deref().unwrap_or(route_namespace);
        match lookup(namespace, &reference.name) {
            Some(state) => {
                count += state.active_count;
                if let Some(config) = state.config {
                    configs.push(config);
                }
            }
            None => missing = Some(format!("{namespace}/{}", reference.name)),
        }
    }
    (count, configs, missing)
}

#[cfg(test)]
fn protocol_rule_invalid_reason_with_runtime(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    let (active_count, configs, missing) = source_ai_admission(rule, route_namespace, |namespace, name| {
        ai_proxy_admission_state(runtime, namespace, name)
    });
    let configs: Vec<_> = configs.iter().collect();
    // Production parse prepares the sidecar after registering dependencies.
    // Direct status callers still need intrinsic diagnostics from current sources.
    let fallback;
    let resolved = match rule.resolved_ai_admission.as_ref() {
        Some(resolved) => resolved,
        None => {
            fallback = resolve_ai_admission(rule, route_namespace, runtime);
            &fallback
        }
    };
    shared_ai_admission::ai_rule_invalid_reason(
        rule,
        route_namespace,
        Some(resolved),
        AiProxyAdmission {
            active_count,
            configs: &configs,
            missing_reference: missing.as_deref(),
        },
    )
}

#[cfg(test)]
fn runtime_ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    protocol_rule_invalid_reason_with_runtime(rule, namespace, runtime)
}

#[cfg(test)]
fn backend_variant_invalid_reason(
    rule: &HTTPRouteRule,
    namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    shared_ai_admission::backend_variant_invalid_reason(rule, &resolve_ai_admission(rule, namespace, runtime))
}

#[cfg(test)]
fn model_route_context_invalid_reason_for_config(
    rule: &HTTPRouteRule,
    namespace: &str,
    runtime: &ControllerRuntime,
    config: &AiProxyConfig,
) -> Option<String> {
    shared_ai_admission::model_route_context_invalid_reason(
        rule,
        namespace,
        &resolve_ai_admission(rule, namespace, runtime),
        config,
    )
}

#[cfg(test)]
fn ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    namespace: &str,
    lookup: impl FnMut(&str, &str) -> Option<AiProxyAdmissionState>,
) -> Option<String> {
    let (active_count, configs, missing) = source_ai_admission(rule, namespace, lookup);
    let configs: Vec<_> = configs.iter().collect();
    let facts = ResolvedAiAdmission {
        backends: rule
            .backend_refs
            .iter()
            .flatten()
            .map(|backend| match backend.backend_kind() {
                HTTPBackendRefKind::Service => ResolvedForwardingTarget::Service,
                HTTPBackendRefKind::EdgionBackend => ResolvedForwardingTarget::EdgionBackend {
                    spec_valid: true,
                    canonical_models: vec![],
                },
                _ => ResolvedForwardingTarget::Missing,
            })
            .collect(),
    };
    shared_ai_admission::ai_rule_invalid_reason(
        rule,
        namespace,
        Some(&facts),
        AiProxyAdmission {
            active_count,
            configs: &configs,
            missing_reference: missing.as_deref(),
        },
    )
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
        if let Some(reason) = backend_waf_invalid_reason(rule, route_ns, runtime) {
            details.push((index, reason));
            continue;
        }
    }
    details.sort_by_key(|(index, _)| *index);
    details
}

fn backend_waf_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    rule.backend_refs
        .iter()
        .flatten()
        .enumerate()
        .find_map(|(index, backend_ref)| {
            backend_ref.filters.iter().flatten().find_map(|filter| {
                if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
                    return None;
                }
                filter.extension_ref.as_ref().and_then(|extension_ref| {
                    super::route_utils::backend_request_scope_extension_ref_error(
                        runtime,
                        route_namespace,
                        index,
                        extension_ref,
                    )
                })
            })
        })
}

fn backend_waf_ownership_errors(
    route: &HTTPRoute,
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
            backend_waf_invalid_reason(rule, route_namespace, runtime).map(|message| AcceptedError::UnsupportedValue {
                message: format!("rules[{index}]: {message}"),
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
        if let Some(reason) = rule.request_mirror_validation_error() {
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
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
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
        let handler = HttpRouteHandler::default();
        let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
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

        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::new()),
            runtime.clone(),
        ));
        let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
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

    fn runtime_with_ai_backends(raw_backends: &[(&str, serde_json::Value)]) -> Arc<ControllerRuntime> {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};

        let runtime = Arc::new(ControllerRuntime::default());
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

    fn invalid_ai_backend_spec() -> edgion_resources::resources::edgion_backend::EdgionBackendSpec {
        serde_json::from_value(serde_json::json!({
            "ai": {
                "provider": "OpenAI",
                "credentialPool": {"credentials": [{
                    "name": "primary", "secretRef": {"name": "provider-key"}
                }]},
                "models": []
            }
        }))
        .expect("structurally deserializable invalid AI backend")
    }

    #[tokio::test]
    async fn ai_rule_preserves_a_missing_service_as_an_invalid_share() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::{EdgionPluginsHandler, ServiceHandler};
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_plugins::{AiProxyConfig, EdgionPluginsSpec, RequestFilterEntry};
        use k8s_openapi::api::core::v1::Service;

        let runtime = runtime_with_ai_backends(&[(
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
            Arc::new(EdgionPluginsHandler),
            runtime.clone(),
        ));
        let mut plugins = EdgionPlugins::new(
            "ai",
            EdgionPluginsSpec {
                current_status: None,
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
    async fn ai_source_errors_do_not_claim_gateway_api_rule_drops() {
        for with_sibling in [false, true] {
            let mut route = route_with_bad_ai_rule(with_sibling);
            let ctx = make_ctx_with_raw_ai_backend("default", "chat");
            HttpRouteHandler::default().update_status(&mut route, &ctx, &[]);
            let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
            let accepted = conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap();
            assert!(!accepted.message.contains("retry.attempts"), "{accepted:?}");
            assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
            assert_eq!(route.spec.invalid_rule_indices, None);
        }
    }

    #[tokio::test]
    async fn ai_admission_facts_replace_injected_metadata_after_flattening() {
        let ctx = make_ctx_with_raw_ai_backend("default", "chat");
        let mut route = route_with_bad_ai_rule(false);
        let rule = &mut route.spec.rules.as_mut().unwrap()[0];
        let mut zero = rule.backend_refs.as_ref().unwrap()[0].clone();
        zero.weight = Some(0);
        rule.backend_refs.as_mut().unwrap().push(zero);
        rule.resolved_ai_admission = Some(ResolvedAiAdmission {
            backends: vec![ResolvedForwardingTarget::Incompatible],
        });
        // An empty wildcard delegation makes the original AI rule a flattened
        // sibling. Its intrinsic retry error must not erase the future gate.
        route.spec.rules.as_mut().unwrap().push(ai_rule(
            serde_json::json!([{
                "group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "*", "namespace": "children"
            }]),
            serde_json::json!([]),
        ));
        let ProcessResult::Continue(mut parsed) = HttpRouteHandler::default().parse(route, &ctx).await else {
            panic!("parse must continue");
        };
        let source = &parsed.spec.rules.as_ref().unwrap()[0];
        let resolved = parsed.spec.resolved_rules.as_ref().unwrap();
        assert_eq!(resolved.len(), 1, "the invalid AI sibling must remain matchable");
        for rule in [source, &resolved[0]] {
            let facts = rule.resolved_ai_admission.as_ref().unwrap();
            assert_eq!(facts.backends.len(), 2, "include the zero-weight slot");
            assert!(facts
                .backends
                .iter()
                .all(|fact| matches!(fact, ResolvedForwardingTarget::EdgionBackend { spec_valid: true, .. })));
        }
        HttpRouteHandler::default().update_status(&mut parsed, &ctx, &[]);
        assert!(!parsed
            .spec
            .invalid_rule_indices
            .as_ref()
            .is_some_and(|indices| indices.contains(&0)));
        let conditions = &parsed.status.as_ref().unwrap().parents[0].conditions;
        assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
        assert!(conditions
            .iter()
            .filter(|condition| condition.type_ == "Accepted")
            .all(|condition| !condition.message.contains("retry.attempts")));
    }

    #[tokio::test]
    async fn ai_source_diagnostic_never_claims_a_rule_drop() {
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx_with_raw_ai_backend("default", "chat");
        let mut route = route_with_bad_ai_rule(true);
        handler.update_status(&mut route, &ctx, &[]);
        route.spec.rules.as_mut().unwrap().truncate(1);
        handler.update_status(&mut route, &ctx, &[]);
        assert!(!route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|condition| condition.type_ == "PartiallyInvalid"));
        assert_eq!(route.spec.invalid_rule_indices, None);

        // Replacing the invalid source with an ordinary Service rule removes
        // the intrinsic AI error without an attachment-policy decision.
        route.spec.rules = Some(vec![ai_rule(
            serde_json::json!([{"name": "web", "port": 80}]),
            serde_json::json!([]),
        )]);
        handler.update_status(&mut route, &ctx, &[]);
        assert!(!route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|condition| condition.type_ == "PartiallyInvalid"));
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
                config: Some(config.clone()),
            }))
            .is_none()
        );
    }

    #[tokio::test]
    async fn positive_weight_backend_requires_ai_capability() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
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
                spec: invalid_ai_backend_spec(),
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
    async fn model_route_checks_zero_weight_slot_identity_before_ignoring_target() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
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
                spec: invalid_ai_backend_spec(),
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
                "unsupported",
                serde_json::json!({"group": "edgion.io", "kind": "UnsupportedBackend", "name": "models"}),
                candidate_config("edgion.io", "UnsupportedBackend", "models", 0),
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
        assert_eq!(
            model_route_context_invalid_reason_for_config(&rule, "apps", &runtime, &config),
            None,
            "a selected zero-weight source remains inert after slot and kind checks"
        );
    }

    #[tokio::test]
    async fn ai_source_diagnostics_use_enabled_processed_entries_without_policy() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use edgion_resources::resources::edgion_plugins::{EdgionPluginsSpec, RequestFilterEntry};
        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler),
            runtime.clone(),
        ));
        let config: AiProxyConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        let mut plugins = EdgionPlugins::new(
            "ai",
            EdgionPluginsSpec {
                current_status: None,
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::AiProxy(config.clone()))]),
                ..Default::default()
            },
        );
        plugins.metadata.namespace = Some("apps".to_string());
        processor.save(plugins.clone(), None);
        runtime.processor_registry.register(processor.clone());
        assert_eq!(
            ai_proxy_admission_state(&runtime, "apps", "ai"),
            Some(AiProxyAdmissionState {
                active_count: 1,
                config: Some(config),
            })
        );
        plugins.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        processor.save(plugins, None);
        assert_eq!(
            ai_proxy_admission_state(&runtime, "apps", "ai"),
            Some(AiProxyAdmissionState::default())
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
                        "name": "models-a", "namespace": "child",
                        "filters": [{
                            "type": "ExtensionRef",
                            "extensionRef": {
                                "group": "edgion.io", "kind": "EdgionPlugins",
                                "name": "backend-plugins", "resolvedNamespace": "child"
                            }
                        }]
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
            vec![
                "child/ai-plugins".to_string(),
                "child/backend-plugins".to_string(),
                "infra/parent-plugins".to_string()
            ]
        );

        let resolved = route.spec.resolved_rules.as_mut().unwrap();
        resolved[0].backend_refs.as_mut().unwrap()[0].name = "models-b".to_string();
        resolved[0].backend_refs.as_mut().unwrap()[0].filters = None;
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

    #[tokio::test]
    async fn backend_waf_is_controller_invalid_and_registered_for_requeue() {
        let ctx = make_ctx_with_raw_waf("apps", "waf");
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
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
        .expect("HTTPRoute fixture must deserialize");

        let details = find_controller_invalid_rule_details(&route, "apps", ctx.runtime());
        assert_eq!(details.len(), 1);
        assert!(details[0].1.contains("cannot contain WAF"), "{:?}", details[0]);
        let accepted = backend_waf_ownership_errors(&route, "apps", ctx.runtime());
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].reason(), condition_reasons::UNSUPPORTED_VALUE);
        assert!(accepted[0].message().contains("rules[0]"));

        HttpRouteHandler::register_plugin_refs(&route, &ctx);
        let route_ref = HttpRouteHandler::create_resource_ref(&route);
        assert_eq!(
            ctx.runtime().plugin_ref_mgr.get_dependencies(&route_ref.key()),
            vec!["apps/waf".to_string()]
        );
    }

    #[tokio::test]
    async fn parse_clears_authored_extension_ref_namespace_metadata() {
        let ctx = make_ctx();
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
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
        .expect("HTTPRoute fixture must deserialize");

        let parsed = match HttpRouteHandler::default().parse(route, &ctx).await {
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
    fn native_cors_v15_validation_reasons_reach_controller_rule_status() {
        for (cors, expected_field) in [
            (
                serde_json::json!({"allowOrigins": ["ftp://example.com"]}),
                "allowOrigins",
            ),
            (serde_json::json!({"allowMethods": ["get"]}), "allowMethods"),
            (serde_json::json!({"allowHeaders": ["bad header"]}), "allowHeaders"),
            (serde_json::json!({"exposeHeaders": [":method"]}), "exposeHeaders"),
            (serde_json::json!({"maxAge": 0}), "maxAge"),
        ] {
            let route: HTTPRoute = serde_json::from_value(serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "HTTPRoute",
                "metadata": {"name": "r1", "namespace": "default"},
                "spec": {"rules": [{"filters": [{"type": "CORS", "cors": cors}]}]}
            }))
            .expect("HTTPRoute CORS fixture must deserialize");
            let details = find_invalid_rule_details(&route, "default");
            assert_eq!(indices_only(details.clone()), vec![0]);
            assert!(
                details[0].1.contains(expected_field),
                "expected {expected_field} in controller diagnostic: {}",
                details[0].1
            );
        }
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
    fn credentialed_expose_headers_status_rejection_and_recovery() {
        let mut route = route_with_one_cors_rule(&["https://app.example"]);
        route.spec.parent_refs = Some(serde_json::from_value(serde_json::json!([{"name": "gw"}])).unwrap());
        let valid = route.spec.rules.as_ref().unwrap()[0].clone();
        let config = route.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
            .cors
            .as_mut()
            .unwrap();
        config.allow_credentials = Some(true);
        config.expose_headers = Some(vec!["*".into()]);
        route.spec.rules.as_mut().unwrap().push(valid);
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx();
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|c| c.type_ == "PartiallyInvalid"
            && c.status == "True"
            && c.message.contains("exposeHeaders")
            && c.message.contains("1 of 2 rules active")));
        route.spec.rules.as_mut().unwrap().truncate(1);
        handler.update_status(&mut route, &ctx, &[]);
        assert!(route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|c| c.type_ == "Accepted"
                && c.status == "False"
                // This unit fixture has no Gateway; its missing-parent reason takes precedence.
                && c.reason == "NoMatchingParent"
                && c.message.contains("exposeHeaders")));
        route.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
            .cors
            .as_mut()
            .unwrap()
            .expose_headers = Some(vec!["X-Request-Id".into()]);
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices, None);
        assert!(!route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|c| c.message.contains("exposeHeaders")));
    }

    #[test]
    fn backend_cors_scope_status_clears_after_policy_moves_to_rule() {
        let mut route = route_with_one_cors_rule(&["https://app.example"]);
        route.spec.parent_refs = Some(serde_json::from_value(serde_json::json!([{"name": "gw"}])).unwrap());
        let rules = route.spec.rules.as_mut().unwrap();
        let valid = rules[0].clone();
        rules[0].backend_refs = Some(
            serde_json::from_value(serde_json::json!([
                {"name": "svc", "port": 80, "filters": [
                    {"type": "CORS", "cors": {"allowOrigins": ["https://app.example"]}}
                ]}
            ]))
            .unwrap(),
        );
        rules.push(valid);
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx();
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0][..]));
        let partial = route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .find(|condition| condition.type_ == "PartiallyInvalid")
            .unwrap();
        assert_eq!(partial.status, "True");
        assert_eq!(partial.reason, "RuleInvalid");
        assert!(partial.message.contains("backendRefs[0] CORS is not supported"));
        assert!(partial.message.contains("1 of 2 rules active"));

        // Removing the valid sibling must not clear the diagnostic as AllRulesValid.
        route.spec.rules.as_mut().unwrap().truncate(1);
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert!(accepted.message.contains("backendRefs[0] CORS is not supported"));
        assert!(!conditions.iter().any(|c| c.type_ == "PartiallyInvalid"));

        route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].filters = None;
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices, None);
        assert!(!route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|condition| condition.message.contains("CORS is not supported")));
        assert!(!route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|condition| condition.type_ == "PartiallyInvalid" && condition.status == "True"));
    }

    #[test]
    fn backend_cors_scope_delegation_diagnostic_survives_attachment_rejection() {
        let mut route = route_with_one_cors_rule(&["https://app.example"]);
        let valid = route.spec.rules.as_ref().unwrap()[0].clone();
        route.spec.parent_refs = Some(serde_json::from_value(serde_json::json!([{"name": "gw"}])).unwrap());
        route.spec.rules = Some(
            serde_json::from_value(serde_json::json!([{
                "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "child"}]
            }]))
            .unwrap(),
        );
        route.spec.resolved_rules = Some(vec![]);
        let diagnostic = "child 'default/child' rules[0] dropped: backendRefs[0] CORS is not supported; configure CORS in rules[].filters instead";
        route.spec.delegation_issues = vec![DelegationIssue::RuleDropped {
            message: diagnostic.to_string(),
        }];
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx();
        handler.update_status(&mut route, &ctx, &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert!(accepted.message.contains(diagnostic));
        assert!(!conditions.iter().any(|c| c.reason == "AllRulesValid"));

        // With a surviving effective rule, diagnostics belong to PartiallyInvalid.
        route.spec.resolved_rules = Some(vec![valid]);
        handler.update_status(&mut route, &ctx, &[]);
        assert!(route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|c| c.type_ == "PartiallyInvalid" && c.status == "True" && c.message.contains(diagnostic)));
        route.spec.delegation_issues.clear();
        handler.update_status(&mut route, &ctx, &[]);
        assert!(!route.status.as_ref().unwrap().parents[0]
            .conditions
            .iter()
            .any(|c| c.message.contains("CORS is not supported")));
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
    fn header_modifier_status_preserves_siblings_rejects_empty_route_and_recovers() {
        let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"name": "headers", "namespace": "default"},
            "spec": {"parentRefs": [{"name": "gw"}], "rules": [
                {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                    "set": [{"name": "bad name", "value": "secret-marker"}]
                }}]},
                {"backendRefs": [{"name": "svc", "port": 80, "filters": [
                    {"type": "ResponseHeaderModifier"}
                ]}]},
                {"filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                    "set": [{"name": "x-valid", "value": "yes"}]
                }}]}
            ]}
        }))
        .unwrap();
        let handler = HttpRouteHandler::default();
        let ctx = make_ctx();
        handler.update_status(&mut route, &ctx, &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        let partial = conditions.iter().find(|c| c.type_ == "PartiallyInvalid").unwrap();
        assert_eq!(partial.status, "True");
        assert!(partial.message.contains("rules[0]") && partial.message.contains("rules[1]"));
        assert!(partial.message.starts_with("Dropped Rule"));
        assert!(partial.message.contains("1 of 3 rules active"));
        assert!(!partial.message.contains("secret-marker"));

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
        // A valid EdgionPlugins ExtensionRef must use the explicit edgion.io group.
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
    fn extension_ref_pairing_errors_drop_only_the_affected_http_rules() {
        let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"name": "extensions", "namespace": "default"},
            "spec": {"parentRefs": [{"name": "gw"}], "rules": [
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
            ]}
        }))
        .expect("HTTPRoute fixture must deserialize");

        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details), vec![0, 1, 2]);

        HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1, 2][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == "PartiallyInvalid"
                && condition.status == "True"
                && condition.message.starts_with("Dropped Rule")
        }));

        route.spec.rules.as_mut().unwrap().truncate(3);
        HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == "Accepted" && condition.status == "False" && condition.message.contains("rules[0]")
        }));
        assert!(!conditions.iter().any(|condition| condition.type_ == "PartiallyInvalid"));
    }

    #[test]
    fn extension_ref_empty_group_is_not_edgion_io() {
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"name": "r", "namespace": "default"},
            "spec": {"rules": [{"filters": [{"type": "ExtensionRef", "extensionRef": {
                "kind": "EdgionPlugins", "name": "plugins"
            }}]}]}
        }))
        .expect("HTTPRoute fixture must deserialize");
        assert_eq!(indices_only(find_invalid_rule_details(&route, "default")), vec![0]);
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
        assert!(details[0].1.contains("httpRouteFilterRequestRedirect"));
        assert!(details[0].1.contains("httpRouteFilterRequestRewrite"));
    }

    #[test]
    fn request_transform_validation_preserves_valid_siblings() {
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "transforms", "namespace": "default"},
            "spec": {"parentRefs": [{"name": "gateway"}], "rules": [
                {"filters": [{"type": "URLRewrite", "urlRewrite": {
                    "path": {"type": "ReplaceFullPath", "replaceFullPath": "/v1"}
                }}]},
                {"matches": [{"path": {"type": "Exact", "value": "/api"}}],
                 "filters": [{"type": "URLRewrite", "urlRewrite": {
                    "path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"}
                 }}]},
                {"backendRefs": [{"name": "backend", "filters": [
                    {"type": "RequestRedirect", "requestRedirect": {"scheme": "ftp"}}
                ]}]},
                {"filters": [{"type": "RequestRedirect", "requestRedirect": {
                    "scheme": "https", "statusCode": 308
                }}]}
            ]}
        }))
        .unwrap();
        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![1, 2]);
        assert!(details[0].1.contains("PathPrefix"));

        let mut route = route;
        HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[1, 2][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == "PartiallyInvalid"
                && condition.status == "True"
                && condition.message.contains("2 of 4 rules active")
        }));
    }

    #[test]
    fn invalid_request_mirror_fields_drop_only_the_owning_rules() {
        let route: HTTPRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "mirrors", "namespace": "default"},
            "spec": {"parentRefs": [{"name": "gateway"}], "rules": [
                {"filters": [{"type": "RequestMirror", "requestMirror": {
                    "backendRef": {"name": "shadow", "port": 8080},
                    "fraction": {"numerator": 1}, "percent": 50
                }}]},
                {"backendRefs": [{"name": "primary", "port": 8080, "filters": [
                    {"type": "RequestMirror", "requestMirror": {
                        "backendRef": {"name": "shadow"}
                    }}
                ]}]},
                {"filters": [{"type": "RequestMirror", "requestMirror": {
                    "backendRef": {"name": "shadow", "port": 8080}, "percent": 100
                }}]}
            ]}
        }))
        .unwrap();

        let details = find_invalid_rule_details(&route, "default");
        assert_eq!(indices_only(details.clone()), vec![0, 1]);
        assert!(details[0].1.contains("only one of fraction or percent"));
        assert!(details[1].1.contains("backendRef.port is required"));

        let mut route = route;
        HttpRouteHandler::default().update_status(&mut route, &make_ctx(), &[]);
        assert_eq!(route.spec.invalid_rule_indices.as_deref(), Some(&[0, 1][..]));
        let conditions = &route.status.as_ref().unwrap().parents[0].conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == "PartiallyInvalid"
                && condition.status == "True"
                && condition.message.contains("1 of 3 rules active")
        }));
    }
}

// Locks the dropped-parent-gateway requeue on the coalesced apply path; see
// `tests/coalesced_recreate_test.rs`.
#[cfg(test)]
#[path = "tests/coalesced_recreate_test.rs"]
mod coalesced_recreate_test;

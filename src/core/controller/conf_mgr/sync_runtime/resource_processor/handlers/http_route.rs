//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use super::super::ref_grant::{
    get_global_cross_ns_ref_manager, is_cross_ns_ref_allowed, validate_http_route_if_enabled,
};
use super::{
    remove_from_attached_route_tracker, remove_from_gateway_route_index, requeue_parent_gateways,
    update_gateway_route_index,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, set_parent_conditions_full, update_condition,
    AcceptedError, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::prelude_resources::HTTPRoute;
use crate::types::resources::common::RefDenied;
use crate::types::resources::http_route::{HTTPRouteFilterType, HTTPRouteStatus, RouteParentStatus};
use crate::types::ResourceKind;

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

    /// Register Service backend references for cross-resource requeue
    fn register_service_refs(route: &HTTPRoute) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        let mut backend_refs_list = Vec::new();
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &rule.backend_refs {
                    for br in backend_refs {
                        backend_refs_list.push((br.kind.as_deref(), br.namespace.as_deref(), br.name.as_str()));
                    }
                }
            }
        }

        super::route_utils::register_service_backend_refs(
            ResourceKind::HTTPRoute,
            route_ns,
            route_name,
            &backend_refs_list,
        );
    }

    /// Record cross-namespace references from backend_refs
    fn record_cross_ns_refs(route: &HTTPRoute) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = Self::create_resource_ref(route);
        let manager = get_global_cross_ns_ref_manager();

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
    }
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
        // Note: per-rule validation (backend refs, filter conflicts) is handled by
        // find_invalid_rule_indices() in update_status(), which marks individual rules
        // as invalid via PartiallyInvalid condition rather than rejecting the entire route.
        errors
    }

    fn preparse(&self, route: &mut HTTPRoute, _ctx: &HandlerContext) -> Vec<String> {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        update_gateway_route_index(
            ResourceKind::HTTPRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
        );
        Vec::new()
    }

    async fn parse(&self, mut route: HTTPRoute, _ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route);

        // Register Service backend references for cross-resource requeue
        Self::register_service_refs(&route);

        // Treat resolved_ports as controller-derived state.
        // Clear any user-provided or stale value before recomputing from current parentRefs.
        route.spec.resolved_ports = None;

        // Resolve effective hostnames (intersection with Gateway listener hostnames)
        if let Some(parent_refs) = &route.spec.parent_refs {
            let route_ns_resolve = route.metadata.namespace.as_deref().unwrap_or("default");
            let resolved = super::hostname_resolution::resolve_effective_hostnames(
                route.spec.hostnames.as_ref(),
                parent_refs,
                route_ns_resolve,
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

        // Resolve listener ports from parentRefs for per-port route isolation
        if let Some(parent_refs) = &route.spec.parent_refs {
            let route_ns_ports = route.metadata.namespace.as_deref().unwrap_or("default");
            let mut ports = Vec::new();
            for parent_ref in parent_refs {
                if let Some(port) = parent_ref.port {
                    ports.push(port as u16);
                } else {
                    let gw_ns = parent_ref
                        .namespace
                        .as_deref()
                        .or(route.metadata.namespace.as_deref())
                        .unwrap_or("default");
                    if let Some(gateway) = super::route_utils::lookup_gateway(gw_ns, &parent_ref.name) {
                        if let Some(listeners) = &gateway.spec.listeners {
                            if let Some(section_name) = &parent_ref.section_name {
                                if let Some(listener) = listeners.iter().find(|l| l.name == *section_name) {
                                    if super::route_utils::listener_allows_route_namespace(
                                        &listener.allowed_routes,
                                        route_ns_ports,
                                        gw_ns,
                                    ) {
                                        ports.push(listener.port as u16);
                                    }
                                }
                            } else {
                                for listener in listeners {
                                    if super::route_utils::listener_allows_route_namespace(
                                        &listener.allowed_routes,
                                        route_ns_ports,
                                        gw_ns,
                                    ) {
                                        ports.push(listener.port as u16);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !ports.is_empty() {
                ports.sort_unstable();
                ports.dedup();
                route.spec.resolved_ports = Some(ports);
            }
        }

        // Mark denied cross-namespace references
        // This sets ref_denied field on BackendRef, which Gateway uses to deny requests
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        if let Some(rules) = &mut route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &mut rule.backend_refs {
                    for backend_ref in backend_refs {
                        if let Some(backend_ns) = &backend_ref.namespace {
                            if backend_ns != route_ns {
                                let allowed = is_cross_ns_ref_allowed(
                                    route_ns,
                                    "HTTPRoute",
                                    backend_ns,
                                    backend_ref.group.as_ref(),
                                    backend_ref.kind.as_ref(),
                                    &backend_ref.name,
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

        ProcessResult::Continue(route)
    }

    async fn on_change(&self, route: &HTTPRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");

        update_gateway_route_index(
            ResourceKind::HTTPRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
        );

        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
    }

    async fn on_delete(&self, route: &HTTPRoute, ctx: &HandlerContext) {
        let resource_ref = Self::create_resource_ref(route);
        get_global_cross_ns_ref_manager().clear_resource_refs(&resource_ref);

        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        super::route_utils::clear_service_backend_refs(ResourceKind::HTTPRoute, route_ns, route_name);

        remove_from_gateway_route_index(ResourceKind::HTTPRoute, route_ns, route_name);

        let tracker_changed = remove_from_attached_route_tracker(ResourceKind::HTTPRoute, route_ns, route_name);
        if tracker_changed {
            requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx).await;
        }
    }

    fn update_status(&self, route: &mut HTTPRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

        let backend_ref_tuples = collect_backend_ref_tuples(route);
        let mut resolved_refs_errors = super::route_utils::validate_backend_refs(route_ns, &backend_ref_tuples);
        let ref_denied_list = collect_ref_denied_list(route);
        resolved_refs_errors.extend(super::route_utils::collect_ref_denied_errors(&ref_denied_list));

        // Per-rule validation: find rules where ALL backend refs are invalid
        let invalid_indices = find_invalid_rule_indices(route, route_ns);
        let total_rules = route.spec.rules.as_ref().map_or(0, |r| r.len());
        let valid_rules = total_rules - invalid_indices.len();

        route.spec.invalid_rule_indices = if invalid_indices.is_empty() {
            None
        } else {
            Some(invalid_indices.clone())
        };

        let all_rules_invalid = total_rules > 0 && valid_rules == 0;

        let status = route.status.get_or_insert_with(|| HTTPRouteStatus { parents: vec![] });
        let route_hostnames = route.spec.hostnames.as_ref();

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let accepted_result =
                    super::route_utils::validate_parent_ref_accepted(route_ns, parent_ref, route_hostnames, "HTTPRoute");
                let mut accepted_errors = accepted_result.errors;
                accepted_errors.extend(validation_accepted_errors.clone());

                if all_rules_invalid {
                    accepted_errors.push(AcceptedError::Invalid {
                        message: format!("All {} rules are invalid", total_rules),
                    });
                }

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
                    &mut status.parents.last_mut().unwrap().conditions
                };

                // Set PartiallyInvalid condition
                if !invalid_indices.is_empty() && valid_rules > 0 {
                    update_condition(
                        conditions,
                        condition_true(
                            condition_types::PARTIALLY_INVALID,
                            condition_reasons::RULE_INVALID,
                            format!(
                                "Rules at indices {:?} are invalid; {} of {} rules are active",
                                invalid_indices, valid_rules, total_rules
                            ),
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
        );
    }
}

/// Extract (kind, namespace, name) tuples from all backend refs for shared validation.
fn collect_backend_ref_tuples(route: &HTTPRoute) -> Vec<(Option<&str>, Option<&str>, &str)> {
    let mut tuples = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for br in backend_refs {
                    tuples.push((br.kind.as_deref(), br.namespace.as_deref(), br.name.as_str()));
                }
            }
        }
    }
    tuples
}

/// Collect Option<&RefDenied> from all backend refs for shared error collection.
fn collect_ref_denied_list(route: &HTTPRoute) -> Vec<Option<&RefDenied>> {
    let mut list = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for br in backend_refs {
                    list.push(br.ref_denied.as_ref());
                }
            }
        }
    }
    list
}

/// Check if a filter list contains both URLRewrite and RequestRedirect.
///
/// Per Gateway API spec, these two filters are mutually exclusive within a single rule.
fn has_rewrite_redirect_conflict(filters: Option<&Vec<crate::types::resources::http_route::HTTPRouteFilter>>) -> bool {
    let Some(filters) = filters else { return false };
    let has_rewrite = filters.iter().any(|f| f.filter_type == HTTPRouteFilterType::URLRewrite);
    let has_redirect = filters
        .iter()
        .any(|f| f.filter_type == HTTPRouteFilterType::RequestRedirect);
    has_rewrite && has_redirect
}

/// Find rule indices where the rule is invalid.
///
/// A rule is invalid if:
/// - It has backend refs and every one is unreachable (wrong kind, missing service, or cross-namespace ref denied)
/// - It has conflicting URLRewrite + RequestRedirect filters
fn find_invalid_rule_indices(route: &HTTPRoute, route_ns: &str) -> Vec<usize> {
    let mut invalid_indices = Vec::new();
    if let Some(rules) = &route.spec.rules {
        for (idx, rule) in rules.iter().enumerate() {
            // Filter conflict makes the entire rule invalid:
            // - Rule-level conflict (both URLRewrite and RequestRedirect on the rule)
            // - Per-backend conflict (both on a single backend ref)
            // - Cross-level conflict (URLRewrite on rule + RequestRedirect on backend, or vice versa)
            if has_rewrite_redirect_conflict(rule.filters.as_ref()) {
                invalid_indices.push(idx);
                continue;
            }
            if let Some(backend_refs) = &rule.backend_refs {
                let backend_conflict = backend_refs.iter().any(|br| {
                    // Check backend ref's own filters
                    if has_rewrite_redirect_conflict(br.filters.as_ref()) {
                        return true;
                    }
                    // Check cross-level: rule filters + backend filters combined
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
                    invalid_indices.push(idx);
                    continue;
                }
            }

            if let Some(backend_refs) = &rule.backend_refs {
                if backend_refs.is_empty() {
                    continue;
                }
                let all_invalid = backend_refs.iter().all(|br| {
                    let kind = br.kind.as_deref().unwrap_or("Service");
                    if kind != "Service" {
                        return true;
                    }
                    let ns = br.namespace.as_deref().unwrap_or(route_ns);
                    if !super::route_utils::service_exists(ns, &br.name) {
                        return true;
                    }
                    // ref_denied backends are NOT treated as invalid here.
                    // They stay in the route table so Gateway can match and return 500.
                    false
                });
                if all_invalid {
                    invalid_indices.push(idx);
                }
            }
        }
    }
    invalid_indices
}

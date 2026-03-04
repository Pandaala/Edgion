//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use super::super::ref_grant::{
    get_global_cross_ns_ref_manager, is_cross_ns_ref_allowed, validate_http_route_if_enabled,
};
use super::{remove_from_attached_route_tracker, requeue_parent_gateways, update_attached_route_tracker};
use crate::core::conf_mgr::sync_runtime::resource_processor::{
    set_route_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult, ProcessorHandler,
    ResolvedRefsError, ResourceRef,
};
use crate::core::conf_mgr::PROCESSOR_REGISTRY;
use crate::types::prelude_resources::HTTPRoute;
use crate::types::resources::common::{ParentReference, RefDenied};
use crate::types::resources::http_route::{HTTPRouteStatus, RouteParentStatus};
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

impl ProcessorHandler<HTTPRoute> for HttpRouteHandler {
    fn validate(&self, route: &HTTPRoute, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = validate_http_route_if_enabled(route);
        errors.extend(validate_backend_refs(route).iter().map(|e| e.message()));
        errors
    }

    fn parse(&self, mut route: HTTPRoute, _ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route);

        // Register Service backend references for cross-resource requeue
        Self::register_service_refs(&route);

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

    fn on_change(&self, route: &HTTPRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        update_attached_route_tracker(
            ResourceKind::HTTPRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
        );
        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx);
    }

    fn on_delete(&self, route: &HTTPRoute, ctx: &HandlerContext) {
        // Clear cross-namespace references when route is deleted
        let resource_ref = Self::create_resource_ref(route);
        get_global_cross_ns_ref_manager().clear_resource_refs(&resource_ref);

        // Clear Service backend references
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        super::route_utils::clear_service_backend_refs(ResourceKind::HTTPRoute, route_ns, route_name);

        remove_from_attached_route_tracker(ResourceKind::HTTPRoute, route_ns, route_name);
        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx);
    }

    fn update_status(&self, route: &mut HTTPRoute, _ctx: &HandlerContext, _validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");

        let mut resolved_refs_errors = validate_backend_refs(route);
        collect_ref_denied_errors(route, &mut resolved_refs_errors);

        let status = route.status.get_or_insert_with(|| HTTPRouteStatus { parents: vec![] });
        let route_hostnames = route.spec.hostnames.as_ref();

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let accepted_errors =
                    validate_parent_ref_accepted_with_hostnames(route_ns, parent_ref, route_hostnames);

                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name
                        && ps.parent_ref.namespace == parent_ref.namespace
                        && ps.parent_ref.section_name == parent_ref.section_name
                });

                if let Some(ps) = parent_status {
                    set_route_parent_conditions_full(
                        &mut ps.conditions,
                        &accepted_errors,
                        &resolved_refs_errors,
                        generation,
                    );
                } else {
                    let mut conditions = Vec::new();
                    set_route_parent_conditions_full(
                        &mut conditions,
                        &accepted_errors,
                        &resolved_refs_errors,
                        generation,
                    );

                    status.parents.push(RouteParentStatus {
                        parent_ref: parent_ref.clone(),
                        controller_name: self.controller_name.clone(),
                        conditions,
                    });
                }
            }
        }
    }
}

/// Validate backend refs and return typed errors for condition generation.
fn validate_backend_refs(route: &HTTPRoute) -> Vec<ResolvedRefsError> {
    let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
    let mut errors = Vec::new();

    let Some(rules) = &route.spec.rules else {
        return errors;
    };

    for rule in rules {
        let Some(backend_refs) = &rule.backend_refs else {
            continue;
        };
        for backend_ref in backend_refs {
            let kind = backend_ref.kind.as_deref().unwrap_or("Service");
            if kind != "Service" {
                errors.push(ResolvedRefsError::InvalidKind {
                    kind: kind.to_string(),
                    name: backend_ref.name.clone(),
                });
                continue;
            }

            let backend_ns = backend_ref.namespace.as_deref().unwrap_or(route_ns);
            if !service_exists(backend_ns, &backend_ref.name) {
                errors.push(ResolvedRefsError::BackendNotFound {
                    namespace: backend_ns.to_string(),
                    name: backend_ref.name.clone(),
                });
            }
        }
    }

    errors
}

/// Collect RefNotPermitted errors from ref_denied fields on backend refs.
fn collect_ref_denied_errors(route: &HTTPRoute, errors: &mut Vec<ResolvedRefsError>) {
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(ref_denied) = &backend_ref.ref_denied {
                        errors.push(ResolvedRefsError::RefNotPermitted {
                            target_namespace: ref_denied.target_namespace.clone(),
                            target_name: ref_denied.target_name.clone(),
                        });
                    }
                }
            }
        }
    }
}

fn service_exists(namespace: &str, name: &str) -> bool {
    let Some(processor) = PROCESSOR_REGISTRY.get("Service") else {
        return true;
    };
    processor.contains_key(&format!("{}/{}", namespace, name))
}

fn validate_parent_ref_accepted_with_hostnames(
    route_ns: &str,
    parent_ref: &ParentReference,
    route_hostnames: Option<&Vec<String>>,
) -> Vec<AcceptedError> {
    let mut errors = Vec::new();

    let parent_group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
    if parent_group != "gateway.networking.k8s.io" {
        return errors;
    }
    let parent_kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
    if parent_kind != "Gateway" {
        return errors;
    }

    let parent_ns = parent_ref.namespace.as_deref().unwrap_or(route_ns);
    let parent_name = &parent_ref.name;

    let Some(gateway) = super::route_utils::lookup_gateway(parent_ns, parent_name) else {
        return errors;
    };

    let empty_listeners = Vec::new();
    let listeners = gateway.spec.listeners.as_ref().unwrap_or(&empty_listeners);

    if let Some(section_name) = &parent_ref.section_name {
        let has_listener = listeners.iter().any(|l| l.name == *section_name);
        if !has_listener {
            errors.push(AcceptedError::NoMatchingParent {
                section_name: section_name.clone(),
            });
            return errors;
        }
    }

    // Check each relevant listener: namespace policy AND hostname intersection
    let matching_listeners: Vec<_> = listeners
        .iter()
        .filter(|l| parent_ref.section_name.as_ref().is_none_or(|sn| l.name == *sn))
        .collect();

    let ns_allowed = matching_listeners
        .iter()
        .any(|l| super::route_utils::listener_allows_route_namespace(&l.allowed_routes, route_ns, parent_ns));

    if !ns_allowed {
        errors.push(AcceptedError::NotAllowedByListeners {
            route_ns: route_ns.to_string(),
        });
        return errors;
    }

    // Check hostname intersection: at least one listener must intersect with route hostnames
    if let Some(route_hs) = route_hostnames {
        if !route_hs.is_empty() {
            let hostname_match = matching_listeners.iter().any(|listener| {
                match &listener.hostname {
                    // Listener with no hostname accepts all route hostnames
                    None => true,
                    Some(listener_hn) => route_hs
                        .iter()
                        .any(|route_hn| super::route_utils::hostnames_intersect(listener_hn, route_hn)),
                }
            });
            if !hostname_match {
                errors.push(AcceptedError::NoMatchingListenerHostname {
                    hostnames: route_hs.clone(),
                });
            }
        }
    }

    errors
}

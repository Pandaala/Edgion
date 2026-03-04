//! UDPRoute Handler
//!
//! Handles UDPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use super::super::ref_grant::{
    get_global_cross_ns_ref_manager, is_cross_ns_ref_allowed, validate_udp_route_if_enabled,
};
use super::{remove_from_attached_route_tracker, requeue_parent_gateways, update_attached_route_tracker};
use crate::core::conf_mgr::sync_runtime::resource_processor::{
    set_route_parent_conditions, HandlerContext, ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::types::prelude_resources::UDPRoute;
use crate::types::resources::common::RefDenied;
use crate::types::resources::http_route::RouteParentStatus;
use crate::types::resources::udp_route::UDPRouteStatus;
use crate::types::ResourceKind;

/// UDPRoute handler
pub struct UdpRouteHandler {
    controller_name: String,
}

impl UdpRouteHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }

    fn create_resource_ref(route: &UDPRoute) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::UDPRoute,
            route.metadata.namespace.clone(),
            route.metadata.name.clone().unwrap_or_default(),
        )
    }

    /// Register Service backend references for cross-resource requeue
    fn register_service_refs(route: &UDPRoute) {
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
            ResourceKind::UDPRoute,
            route_ns,
            route_name,
            &backend_refs_list,
        );
    }

    /// Record cross-namespace references from backend_refs
    fn record_cross_ns_refs(route: &UDPRoute) {
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

impl Default for UdpRouteHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

impl ProcessorHandler<UDPRoute> for UdpRouteHandler {
    fn validate(&self, route: &UDPRoute, _ctx: &HandlerContext) -> Vec<String> {
        validate_udp_route_if_enabled(route)
    }

    fn parse(&self, mut route: UDPRoute, _ctx: &HandlerContext) -> ProcessResult<UDPRoute> {
        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route);

        // Register Service backend references for cross-resource requeue
        Self::register_service_refs(&route);

        // Mark denied cross-namespace references
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        if let Some(rules) = &mut route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &mut rule.backend_refs {
                    for backend_ref in backend_refs {
                        if let Some(backend_ns) = &backend_ref.namespace {
                            if backend_ns != route_ns {
                                let allowed = is_cross_ns_ref_allowed(
                                    route_ns,
                                    "UDPRoute",
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
                                    backend_ref.ref_denied = None;
                                }
                            } else {
                                backend_ref.ref_denied = None;
                            }
                        } else {
                            backend_ref.ref_denied = None;
                        }
                    }
                }
            }
        }

        ProcessResult::Continue(route)
    }

    fn on_change(&self, route: &UDPRoute, ctx: &HandlerContext) {
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        update_attached_route_tracker(
            ResourceKind::UDPRoute,
            route_ns,
            route_name,
            route.spec.parent_refs.as_ref(),
        );
        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx);
    }

    fn on_delete(&self, route: &UDPRoute, ctx: &HandlerContext) {
        // Clear cross-namespace references when route is deleted
        let resource_ref = Self::create_resource_ref(route);
        get_global_cross_ns_ref_manager().clear_resource_refs(&resource_ref);

        // Clear Service backend references
        let route_ns = route.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = route.metadata.name.as_deref().unwrap_or("");
        super::route_utils::clear_service_backend_refs(ResourceKind::UDPRoute, route_ns, route_name);

        remove_from_attached_route_tracker(ResourceKind::UDPRoute, route_ns, route_name);
        requeue_parent_gateways(route.spec.parent_refs.as_ref(), route_ns, ctx);
    }

    fn update_status(&self, route: &mut UDPRoute, _ctx: &HandlerContext, _validation_errors: &[String]) {
        let generation = route.metadata.generation;

        let mut resolved_refs_errors: Vec<ResolvedRefsError> = Vec::new();
        if let Some(rules) = &route.spec.rules {
            for rule in rules {
                if let Some(backend_refs) = &rule.backend_refs {
                    for backend_ref in backend_refs {
                        if let Some(ref_denied) = &backend_ref.ref_denied {
                            resolved_refs_errors.push(ResolvedRefsError::RefNotPermitted {
                                target_namespace: ref_denied.target_namespace.clone(),
                                target_name: ref_denied.target_name.clone(),
                            });
                        }
                    }
                }
            }
        }

        let status = route.status.get_or_insert_with(|| UDPRouteStatus { parents: vec![] });

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name && ps.parent_ref.namespace == parent_ref.namespace
                });

                if let Some(ps) = parent_status {
                    set_route_parent_conditions(&mut ps.conditions, &resolved_refs_errors, generation);
                } else {
                    let mut conditions = Vec::new();
                    set_route_parent_conditions(&mut conditions, &resolved_refs_errors, generation);

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

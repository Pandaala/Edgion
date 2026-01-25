//! TCPRoute Handler
//!
//! Handles TCPRoute resources with ReferenceGrant validation and cross-namespace reference tracking.

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    set_route_parent_conditions, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::core::ref_grant::{
    get_global_cross_ns_ref_manager, is_cross_ns_ref_allowed, validate_tcp_route_if_enabled, CrossNsResourceRef,
};
use crate::types::prelude_resources::TCPRoute;
use crate::types::resources::common::RefDenied;
use crate::types::resources::http_route::RouteParentStatus;
use crate::types::resources::tcp_route::TCPRouteStatus;
use crate::types::ResourceKind;

/// Controller name for status reporting
const CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

/// TCPRoute handler
pub struct TcpRouteHandler;

impl TcpRouteHandler {
    pub fn new() -> Self {
        Self
    }

    /// Create a CrossNsResourceRef for this route
    fn create_resource_ref(route: &TCPRoute) -> CrossNsResourceRef {
        CrossNsResourceRef::new(
            ResourceKind::TCPRoute,
            route.metadata.namespace.clone(),
            route.metadata.name.clone().unwrap_or_default(),
        )
    }

    /// Record cross-namespace references from backend_refs
    fn record_cross_ns_refs(route: &TCPRoute) {
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
                                manager.add_cross_ns_ref(backend_ns.clone(), resource_ref.clone());
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Default for TcpRouteHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<TCPRoute> for TcpRouteHandler {
    fn validate(&self, route: &TCPRoute, _ctx: &HandlerContext) -> Vec<String> {
        validate_tcp_route_if_enabled(route)
    }

    fn parse(&self, mut route: TCPRoute, _ctx: &HandlerContext) -> ProcessResult<TCPRoute> {
        // Record cross-namespace references for revalidation when ReferenceGrant changes
        Self::record_cross_ns_refs(&route);

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
                                    "TCPRoute",
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
                                }
                            }
                        }
                    }
                }
            }
        }

        ProcessResult::Continue(route)
    }

    fn on_delete(&self, route: &TCPRoute, _ctx: &HandlerContext) {
        // Clear cross-namespace references when route is deleted
        let resource_ref = Self::create_resource_ref(route);
        get_global_cross_ns_ref_manager().clear_resource_refs(&resource_ref);
    }

    fn update_status(&self, route: &mut TCPRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;
        let status = route.status.get_or_insert_with(|| TCPRouteStatus { parents: vec![] });

        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name && ps.parent_ref.namespace == parent_ref.namespace
                });

                if let Some(ps) = parent_status {
                    set_route_parent_conditions(&mut ps.conditions, validation_errors, generation);
                } else {
                    let mut conditions = Vec::new();
                    set_route_parent_conditions(&mut conditions, validation_errors, generation);

                    status.parents.push(RouteParentStatus {
                        parent_ref: parent_ref.clone(),
                        controller_name: CONTROLLER_NAME.to_string(),
                        conditions,
                    });
                }
            }
        }
    }
}

//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation.

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    set_route_parent_conditions, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::core::ref_grant::validate_http_route_if_enabled;
use crate::types::prelude_resources::HTTPRoute;
use crate::types::resources::http_route::{HTTPRouteStatus, RouteParentStatus};

/// Controller name for status reporting
const CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

/// HTTPRoute handler
pub struct HttpRouteHandler;

impl HttpRouteHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HttpRouteHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<HTTPRoute> for HttpRouteHandler {
    fn validate(&self, route: &HTTPRoute, _ctx: &HandlerContext) -> Vec<String> {
        validate_http_route_if_enabled(route)
    }

    fn parse(&self, route: HTTPRoute, _ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        ProcessResult::Continue(route)
    }

    fn update_status(&self, route: &mut HTTPRoute, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = route.metadata.generation;

        // Initialize status if not present
        let status = route.status.get_or_insert_with(|| HTTPRouteStatus { parents: vec![] });

        // Update status for each parent ref
        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                // Find existing parent status or create new one
                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name && ps.parent_ref.namespace == parent_ref.namespace
                });

                if let Some(ps) = parent_status {
                    // Update existing parent status
                    set_route_parent_conditions(&mut ps.conditions, validation_errors, generation);
                } else {
                    // Create new parent status
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

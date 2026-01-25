//! TCPRoute Handler
//!
//! Handles TCPRoute resources with ReferenceGrant validation.

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    set_route_parent_conditions, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::core::ref_grant::validate_tcp_route_if_enabled;
use crate::types::prelude_resources::TCPRoute;
use crate::types::resources::http_route::RouteParentStatus;
use crate::types::resources::tcp_route::TCPRouteStatus;

/// Controller name for status reporting
const CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

/// TCPRoute handler
pub struct TcpRouteHandler;

impl TcpRouteHandler {
    pub fn new() -> Self {
        Self
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

    fn parse(&self, route: TCPRoute, _ctx: &HandlerContext) -> ProcessResult<TCPRoute> {
        ProcessResult::Continue(route)
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

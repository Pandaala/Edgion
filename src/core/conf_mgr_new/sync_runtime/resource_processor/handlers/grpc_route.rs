//! GRPCRoute Handler
//!
//! Handles GRPCRoute resources with ReferenceGrant validation.

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::core::ref_grant::validate_grpc_route_if_enabled;
use crate::types::prelude_resources::GRPCRoute;

/// GRPCRoute handler
pub struct GrpcRouteHandler;

impl GrpcRouteHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GrpcRouteHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<GRPCRoute> for GrpcRouteHandler {
    fn validate(&self, route: &GRPCRoute, _ctx: &HandlerContext) -> Vec<String> {
        validate_grpc_route_if_enabled(route)
    }

    fn parse(&self, route: GRPCRoute, _ctx: &HandlerContext) -> ProcessResult<GRPCRoute> {
        ProcessResult::Continue(route)
    }
}

//! TCPRoute Handler
//!
//! Handles TCPRoute resources with ReferenceGrant validation.

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::core::ref_grant::validate_tcp_route_if_enabled;
use crate::types::prelude_resources::TCPRoute;

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
}

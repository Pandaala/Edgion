//! UDPRoute Handler
//!
//! Handles UDPRoute resources with ReferenceGrant validation.

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::core::ref_grant::validate_udp_route_if_enabled;
use crate::types::prelude_resources::UDPRoute;

/// UDPRoute handler
pub struct UdpRouteHandler;

impl UdpRouteHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UdpRouteHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<UDPRoute> for UdpRouteHandler {
    fn validate(&self, route: &UDPRoute, _ctx: &HandlerContext) -> Vec<String> {
        validate_udp_route_if_enabled(route)
    }

    fn parse(&self, route: UDPRoute, _ctx: &HandlerContext) -> ProcessResult<UDPRoute> {
        ProcessResult::Continue(route)
    }
}

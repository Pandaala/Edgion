//! HTTPRoute Handler
//!
//! Handles HTTPRoute resources with ReferenceGrant validation.

use crate::core::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::core::ref_grant::validate_http_route_if_enabled;
use crate::types::prelude_resources::HTTPRoute;

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
}

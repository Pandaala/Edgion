//! TLSRoute Handler
//!
//! Handles TLSRoute resources with ReferenceGrant validation.

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::core::ref_grant::validate_tls_route_if_enabled;
use crate::types::prelude_resources::TLSRoute;

/// TLSRoute handler
pub struct TlsRouteHandler;

impl TlsRouteHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TlsRouteHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<TLSRoute> for TlsRouteHandler {
    fn validate(&self, route: &TLSRoute, _ctx: &HandlerContext) -> Vec<String> {
        validate_tls_route_if_enabled(route)
    }

    fn parse(&self, route: TLSRoute, _ctx: &HandlerContext) -> ProcessResult<TLSRoute> {
        ProcessResult::Continue(route)
    }
}

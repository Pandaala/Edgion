//! Endpoints Handler
//!
//! Handles Endpoints resources.

use k8s_openapi::api::core::v1::Endpoints;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};

/// Endpoints handler
pub struct EndpointsHandler;

impl EndpointsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EndpointsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<Endpoints> for EndpointsHandler {
    fn parse(&self, ep: Endpoints, _ctx: &HandlerContext) -> ProcessResult<Endpoints> {
        ProcessResult::Continue(ep)
    }
}

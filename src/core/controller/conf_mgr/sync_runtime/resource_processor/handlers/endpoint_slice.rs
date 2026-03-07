//! EndpointSlice Handler
//!
//! Handles EndpointSlice resources.

use k8s_openapi::api::discovery::v1::EndpointSlice;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, ProcessResult, ProcessorHandler,
};

/// EndpointSlice handler
pub struct EndpointSliceHandler;

impl EndpointSliceHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EndpointSliceHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<EndpointSlice> for EndpointSliceHandler {
    fn parse(&self, eps: EndpointSlice, _ctx: &HandlerContext) -> ProcessResult<EndpointSlice> {
        ProcessResult::Continue(eps)
    }
}

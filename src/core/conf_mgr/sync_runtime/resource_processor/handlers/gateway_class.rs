//! GatewayClass Handler
//!
//! Handles GatewayClass resources.

use crate::core::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::GatewayClass;

/// GatewayClass handler
pub struct GatewayClassHandler;

impl GatewayClassHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GatewayClassHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<GatewayClass> for GatewayClassHandler {
    fn parse(&self, gc: GatewayClass, _ctx: &HandlerContext) -> ProcessResult<GatewayClass> {
        ProcessResult::Continue(gc)
    }
}

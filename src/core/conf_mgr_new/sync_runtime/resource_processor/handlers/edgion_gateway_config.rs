//! EdgionGatewayConfig Handler
//!
//! Handles EdgionGatewayConfig resources.

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::EdgionGatewayConfig;

/// EdgionGatewayConfig handler
pub struct EdgionGatewayConfigHandler;

impl EdgionGatewayConfigHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionGatewayConfigHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn parse(&self, egc: EdgionGatewayConfig, _ctx: &HandlerContext) -> ProcessResult<EdgionGatewayConfig> {
        ProcessResult::Continue(egc)
    }
}

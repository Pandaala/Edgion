//! ReferenceGrant Handler
//!
//! Handles ReferenceGrant resources.

use crate::core::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::ReferenceGrant;

/// ReferenceGrant handler
pub struct ReferenceGrantHandler;

impl ReferenceGrantHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReferenceGrantHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<ReferenceGrant> for ReferenceGrantHandler {
    fn parse(&self, rg: ReferenceGrant, _ctx: &HandlerContext) -> ProcessResult<ReferenceGrant> {
        ProcessResult::Continue(rg)
    }
}

//! BackendTLSPolicy Handler
//!
//! Handles BackendTLSPolicy resources.

use crate::core::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::BackendTLSPolicy;

/// BackendTLSPolicy handler
pub struct BackendTlsPolicyHandler;

impl BackendTlsPolicyHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BackendTlsPolicyHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<BackendTLSPolicy> for BackendTlsPolicyHandler {
    fn parse(&self, btp: BackendTLSPolicy, _ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicy> {
        ProcessResult::Continue(btp)
    }
}

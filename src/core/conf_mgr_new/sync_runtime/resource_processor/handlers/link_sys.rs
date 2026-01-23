//! LinkSys Handler
//!
//! Handles LinkSys resources.

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::LinkSys;

/// LinkSys handler
pub struct LinkSysHandler;

impl LinkSysHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinkSysHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<LinkSys> for LinkSysHandler {
    fn parse(&self, ls: LinkSys, _ctx: &HandlerContext) -> ProcessResult<LinkSys> {
        ProcessResult::Continue(ls)
    }
}

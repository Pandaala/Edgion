//! EdgionPlugins Handler
//!
//! Handles EdgionPlugins resources.

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::EdgionPlugins;

/// EdgionPlugins handler
pub struct EdgionPluginsHandler;

impl EdgionPluginsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<EdgionPlugins> for EdgionPluginsHandler {
    fn parse(&self, ep: EdgionPlugins, _ctx: &HandlerContext) -> ProcessResult<EdgionPlugins> {
        ProcessResult::Continue(ep)
    }
}

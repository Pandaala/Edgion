//! EdgionStreamPlugins Handler
//!
//! Handles EdgionStreamPlugins resources.

use crate::core::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::EdgionStreamPlugins;

/// EdgionStreamPlugins handler
pub struct EdgionStreamPluginsHandler;

impl EdgionStreamPluginsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionStreamPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<EdgionStreamPlugins> for EdgionStreamPluginsHandler {
    fn parse(&self, esp: EdgionStreamPlugins, _ctx: &HandlerContext) -> ProcessResult<EdgionStreamPlugins> {
        ProcessResult::Continue(esp)
    }
}

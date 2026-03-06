//! PluginMetadata Handler
//!
//! Handles PluginMetaData resources.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
use crate::types::prelude_resources::PluginMetaData;

/// PluginMetaData handler
pub struct PluginMetadataHandler;

impl PluginMetadataHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PluginMetadataHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<PluginMetaData> for PluginMetadataHandler {
    fn parse(&self, pm: PluginMetaData, _ctx: &HandlerContext) -> ProcessResult<PluginMetaData> {
        ProcessResult::Continue(pm)
    }
}

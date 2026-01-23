//! PluginMetaData Processor
//!
//! Handles PluginMetaData resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::PluginMetaData;

/// PluginMetaData processor
pub struct PluginMetadataProcessor;

impl PluginMetadataProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PluginMetadataProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<PluginMetaData> for PluginMetadataProcessor {
    fn kind(&self) -> &'static str {
        "PluginMetaData"
    }

    fn parse(&self, pm: PluginMetaData, _ctx: &ProcessContext) -> ProcessResult<PluginMetaData> {
        ProcessResult::Continue(pm)
    }

    fn save(&self, cs: &ConfigServer, pm: PluginMetaData) {
        cs.plugin_metadata.apply_change(ResourceChange::EventUpdate, pm);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.plugin_metadata.get_by_key(key) {
            cs.plugin_metadata.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<PluginMetaData> {
        cs.plugin_metadata.get_by_key(key)
    }
}

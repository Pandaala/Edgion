//! EdgionStreamPlugins Processor
//!
//! Handles EdgionStreamPlugins resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::EdgionStreamPlugins;

/// EdgionStreamPlugins processor
pub struct EdgionStreamPluginsProcessor;

impl EdgionStreamPluginsProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionStreamPluginsProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<EdgionStreamPlugins> for EdgionStreamPluginsProcessor {
    fn kind(&self) -> &'static str {
        "EdgionStreamPlugins"
    }

    fn parse(&self, esp: EdgionStreamPlugins, _ctx: &ProcessContext) -> ProcessResult<EdgionStreamPlugins> {
        ProcessResult::Continue(esp)
    }

    fn save(&self, cs: &ConfigServer, esp: EdgionStreamPlugins) {
        cs.edgion_stream_plugins
            .apply_change(ResourceChange::EventUpdate, esp);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.edgion_stream_plugins.get_by_key(key) {
            cs.edgion_stream_plugins
                .apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<EdgionStreamPlugins> {
        cs.edgion_stream_plugins.get_by_key(key)
    }
}

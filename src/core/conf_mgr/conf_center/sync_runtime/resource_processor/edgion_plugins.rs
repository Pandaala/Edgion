//! EdgionPlugins Processor
//!
//! Handles EdgionPlugins resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::EdgionPlugins;

/// EdgionPlugins processor
pub struct EdgionPluginsProcessor;

impl EdgionPluginsProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionPluginsProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<EdgionPlugins> for EdgionPluginsProcessor {
    fn kind(&self) -> &'static str {
        "EdgionPlugins"
    }

    fn parse(&self, ep: EdgionPlugins, _ctx: &ProcessContext) -> ProcessResult<EdgionPlugins> {
        ProcessResult::Continue(ep)
    }

    fn save(&self, cs: &ConfigServer, ep: EdgionPlugins) {
        cs.edgion_plugins.apply_change(ResourceChange::EventUpdate, ep);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.edgion_plugins.get_by_key(key) {
            cs.edgion_plugins.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<EdgionPlugins> {
        cs.edgion_plugins.get_by_key(key)
    }
}

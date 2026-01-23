//! LinkSys Processor
//!
//! Handles LinkSys resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::LinkSys;

/// LinkSys processor
pub struct LinkSysProcessor;

impl LinkSysProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinkSysProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<LinkSys> for LinkSysProcessor {
    fn kind(&self) -> &'static str {
        "LinkSys"
    }

    fn parse(&self, ls: LinkSys, _ctx: &ProcessContext) -> ProcessResult<LinkSys> {
        ProcessResult::Continue(ls)
    }

    fn save(&self, cs: &ConfigServer, ls: LinkSys) {
        cs.link_sys.apply_change(ResourceChange::EventUpdate, ls);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.link_sys.get_by_key(key) {
            cs.link_sys.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<LinkSys> {
        cs.link_sys.get_by_key(key)
    }
}

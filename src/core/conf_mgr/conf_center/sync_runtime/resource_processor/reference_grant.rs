//! ReferenceGrant Processor
//!
//! Handles ReferenceGrant resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::ReferenceGrant;

/// ReferenceGrant processor
pub struct ReferenceGrantProcessor;

impl ReferenceGrantProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReferenceGrantProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<ReferenceGrant> for ReferenceGrantProcessor {
    fn kind(&self) -> &'static str {
        "ReferenceGrant"
    }

    fn parse(&self, rg: ReferenceGrant, _ctx: &ProcessContext) -> ProcessResult<ReferenceGrant> {
        ProcessResult::Continue(rg)
    }

    fn save(&self, cs: &ConfigServer, rg: ReferenceGrant) {
        cs.reference_grants.apply_change(ResourceChange::EventUpdate, rg);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.reference_grants.get_by_key(key) {
            cs.reference_grants.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<ReferenceGrant> {
        cs.reference_grants.get_by_key(key)
    }
}

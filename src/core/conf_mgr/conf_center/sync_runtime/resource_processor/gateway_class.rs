//! GatewayClass Processor
//!
//! Handles GatewayClass resources (cluster-scoped)

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::GatewayClass;

/// GatewayClass processor
pub struct GatewayClassProcessor;

impl GatewayClassProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GatewayClassProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<GatewayClass> for GatewayClassProcessor {
    fn kind(&self) -> &'static str {
        "GatewayClass"
    }

    fn parse(&self, gc: GatewayClass, _ctx: &ProcessContext) -> ProcessResult<GatewayClass> {
        ProcessResult::Continue(gc)
    }

    fn save(&self, cs: &ConfigServer, gc: GatewayClass) {
        cs.gateway_classes.apply_change(ResourceChange::EventUpdate, gc);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.gateway_classes.get_by_key(key) {
            cs.gateway_classes.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<GatewayClass> {
        cs.gateway_classes.get_by_key(key)
    }
}

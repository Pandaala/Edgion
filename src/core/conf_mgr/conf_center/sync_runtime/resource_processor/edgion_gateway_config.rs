//! EdgionGatewayConfig Processor
//!
//! Handles EdgionGatewayConfig resources (cluster-scoped)

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::EdgionGatewayConfig;

/// EdgionGatewayConfig processor
pub struct EdgionGatewayConfigProcessor;

impl EdgionGatewayConfigProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionGatewayConfigProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<EdgionGatewayConfig> for EdgionGatewayConfigProcessor {
    fn kind(&self) -> &'static str {
        "EdgionGatewayConfig"
    }

    fn parse(&self, egc: EdgionGatewayConfig, _ctx: &ProcessContext) -> ProcessResult<EdgionGatewayConfig> {
        ProcessResult::Continue(egc)
    }

    fn save(&self, cs: &ConfigServer, egc: EdgionGatewayConfig) {
        cs.edgion_gateway_configs.apply_change(ResourceChange::EventUpdate, egc);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.edgion_gateway_configs.get_by_key(key) {
            cs.edgion_gateway_configs.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<EdgionGatewayConfig> {
        cs.edgion_gateway_configs.get_by_key(key)
    }
}

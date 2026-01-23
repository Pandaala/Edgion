//! Endpoints Processor
//!
//! Handles Endpoints resources

use k8s_openapi::api::core::v1::Endpoints;

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};

/// Endpoints processor
pub struct EndpointsProcessor;

impl EndpointsProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EndpointsProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<Endpoints> for EndpointsProcessor {
    fn kind(&self) -> &'static str {
        "Endpoints"
    }

    fn parse(&self, ep: Endpoints, _ctx: &ProcessContext) -> ProcessResult<Endpoints> {
        ProcessResult::Continue(ep)
    }

    fn save(&self, cs: &ConfigServer, ep: Endpoints) {
        cs.endpoints.apply_change(ResourceChange::EventUpdate, ep);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.endpoints.get_by_key(key) {
            cs.endpoints.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<Endpoints> {
        cs.endpoints.get_by_key(key)
    }
}

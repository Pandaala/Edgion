//! EndpointSlice Processor
//!
//! Handles EndpointSlice resources

use k8s_openapi::api::discovery::v1::EndpointSlice;

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};

/// EndpointSlice processor
pub struct EndpointSliceProcessor;

impl EndpointSliceProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EndpointSliceProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<EndpointSlice> for EndpointSliceProcessor {
    fn kind(&self) -> &'static str {
        "EndpointSlice"
    }

    fn parse(&self, es: EndpointSlice, _ctx: &ProcessContext) -> ProcessResult<EndpointSlice> {
        ProcessResult::Continue(es)
    }

    fn save(&self, cs: &ConfigServer, es: EndpointSlice) {
        cs.endpoint_slices.apply_change(ResourceChange::EventUpdate, es);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.endpoint_slices.get_by_key(key) {
            cs.endpoint_slices.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<EndpointSlice> {
        cs.endpoint_slices.get_by_key(key)
    }
}

//! Service Processor
//!
//! Handles Service resources

use k8s_openapi::api::core::v1::Service;

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};

/// Service processor
pub struct ServiceProcessor;

impl ServiceProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ServiceProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<Service> for ServiceProcessor {
    fn kind(&self) -> &'static str {
        "Service"
    }

    fn parse(&self, svc: Service, _ctx: &ProcessContext) -> ProcessResult<Service> {
        ProcessResult::Continue(svc)
    }

    fn save(&self, cs: &ConfigServer, svc: Service) {
        cs.services.apply_change(ResourceChange::EventUpdate, svc);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.services.get_by_key(key) {
            cs.services.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<Service> {
        cs.services.get_by_key(key)
    }
}

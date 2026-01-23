//! BackendTLSPolicy Processor
//!
//! Handles BackendTLSPolicy resources

use super::{ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::BackendTLSPolicy;

/// BackendTLSPolicy processor
pub struct BackendTlsPolicyProcessor;

impl BackendTlsPolicyProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BackendTlsPolicyProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<BackendTLSPolicy> for BackendTlsPolicyProcessor {
    fn kind(&self) -> &'static str {
        "BackendTLSPolicy"
    }

    fn parse(&self, btp: BackendTLSPolicy, _ctx: &ProcessContext) -> ProcessResult<BackendTLSPolicy> {
        ProcessResult::Continue(btp)
    }

    fn save(&self, cs: &ConfigServer, btp: BackendTLSPolicy) {
        cs.backend_tls_policies.apply_change(ResourceChange::EventUpdate, btp);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.backend_tls_policies.get_by_key(key) {
            cs.backend_tls_policies.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<BackendTLSPolicy> {
        cs.backend_tls_policies.get_by_key(key)
    }
}

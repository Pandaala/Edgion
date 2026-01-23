//! Service Handler
//!
//! Handles Service resources.

use k8s_openapi::api::core::v1::Service;

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};

/// Service handler
pub struct ServiceHandler;

impl ServiceHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ServiceHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<Service> for ServiceHandler {
    fn parse(&self, svc: Service, _ctx: &HandlerContext) -> ProcessResult<Service> {
        ProcessResult::Continue(svc)
    }
}

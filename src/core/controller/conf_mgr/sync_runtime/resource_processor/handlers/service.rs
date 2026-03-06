//! Service Handler
//!
//! Handles Service resources with cascading requeue for dependent routes.
//! When a Service is created/updated/deleted, routes that reference it as
//! a backend are requeued so their ResolvedRefs status is re-evaluated.

use k8s_openapi::api::core::v1::Service;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::service_ref::get_service_ref_manager;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};

/// Service handler
pub struct ServiceHandler;

impl ServiceHandler {
    pub fn new() -> Self {
        Self
    }

    fn requeue_dependent_routes(&self, svc: &Service, event: &str, ctx: &HandlerContext) {
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let svc_name = svc.metadata.name.as_deref().unwrap_or("");
        let service_key = format!("{}/{}", svc_ns, svc_name);

        let refs = get_service_ref_manager().get_refs(&service_key);
        if refs.is_empty() {
            return;
        }

        tracing::info!(
            service_key = %service_key,
            ref_count = refs.len(),
            event = %event,
            "Service changed, requeuing dependent routes"
        );

        for resource_ref in refs {
            let key = match &resource_ref.namespace {
                Some(ns) => format!("{}/{}", ns, resource_ref.name),
                None => resource_ref.name.clone(),
            };
            ctx.requeue(resource_ref.kind.as_str(), key);
        }
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

    fn on_change(&self, svc: &Service, ctx: &HandlerContext) {
        self.requeue_dependent_routes(svc, "updated", ctx);
    }

    fn on_delete(&self, svc: &Service, ctx: &HandlerContext) {
        self.requeue_dependent_routes(svc, "deleted", ctx);
    }
}

//! EndpointSlice Handler
//!
//! Handles EndpointSlice resources. When an EndpointSlice changes, requeues
//! all registered `service_ref_mgr` dependents (Routes, BackendTLSPolicy) that
//! track the parent Service, and relays the updated EndpointSlice to the gateway
//! via conf_sync so the gateway can resolve endpoints itself.

use k8s_openapi::api::discovery::v1::EndpointSlice;

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};

/// EndpointSlice handler
pub struct EndpointSliceHandler;

impl EndpointSliceHandler {
    pub fn new() -> Self {
        Self
    }

    async fn requeue_dependent_resources(&self, eps: &EndpointSlice, ctx: &HandlerContext) {
        let svc_name = eps
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get("kubernetes.io/service-name"))
            .map(String::as_str)
            .unwrap_or("");
        if svc_name.is_empty() {
            return;
        }
        let ns = eps.metadata.namespace.as_deref().unwrap_or("default");
        let service_key = format!("{ns}/{svc_name}");

        let refs = crate::conf_mgr::sync_runtime::resource_processor::service_ref::service_event_refs(
            &ctx.runtime().service_ref_mgr,
            &service_key,
        );
        if refs.is_empty() {
            return;
        }

        tracing::info!(
            service_key = %service_key,
            ref_count = refs.len(),
            "EndpointSlice changed, requeuing dependent resources"
        );

        for resource_ref in refs {
            let key = match &resource_ref.namespace {
                Some(ns) => format!("{}/{}", ns, resource_ref.name),
                None => resource_ref.name.clone(),
            };
            ctx.requeue(resource_ref.kind.as_str(), key).await;
        }
    }
}

impl Default for EndpointSliceHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EndpointSlice> for EndpointSliceHandler {
    async fn parse(&self, eps: EndpointSlice, _ctx: &HandlerContext) -> ProcessResult<EndpointSlice> {
        ProcessResult::Continue(eps)
    }

    async fn on_change(&self, eps: &EndpointSlice, ctx: &HandlerContext) {
        self.requeue_dependent_resources(eps, ctx).await;
    }

    async fn on_delete(&self, eps: &EndpointSlice, ctx: &HandlerContext) {
        self.requeue_dependent_resources(eps, ctx).await;
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice_test.rs"]
mod tests;

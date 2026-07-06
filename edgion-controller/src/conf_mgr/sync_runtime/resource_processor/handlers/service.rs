//! Service Handler
//!
//! Handles Service resources with cascading requeue for dependent routes.
//! When a Service is created/updated/deleted, routes that reference it as
//! a backend are requeued so their ResolvedRefs status is re-evaluated.

use k8s_openapi::api::core::v1::Service;

use crate::conf_mgr::sync_runtime::resource_processor::handlers::dns_resolver_guard;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};

/// Service handler
pub struct ServiceHandler;

impl ServiceHandler {
    pub fn new() -> Self {
        Self
    }

    async fn requeue_dependent_routes(&self, svc: &Service, event: &str, ctx: &HandlerContext) {
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let svc_name = svc.metadata.name.as_deref().unwrap_or("");
        let service_key = format!("{}/{}", svc_ns, svc_name);

        let refs = ctx.runtime().service_ref_mgr.get_refs(&service_key);
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
            ctx.requeue(resource_ref.kind.as_str(), key).await;
        }
    }
}

impl Default for ServiceHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<Service> for ServiceHandler {
    async fn parse(&self, mut svc: Service, ctx: &HandlerContext) -> ProcessResult<Service> {
        // Gate a cross-namespace `edgion.io/dns-resolver-ref` to an httpdns
        // LinkSys. A native Service has no Edgion status surface, so a denial is
        // logged (not a status condition) and the annotation is stripped so the
        // data plane never honors an unpermitted reference (falls back to the
        // next resolver layer / system).
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default").to_string();
        if let Some(denial) = dns_resolver_guard::check_dns_resolver_ref_grant(
            &svc_ns,
            "",
            "Service",
            svc.metadata.annotations.as_ref(),
            ctx.runtime(),
        ) {
            tracing::warn!(
                service_ns = %svc_ns,
                service = svc.metadata.name.as_deref().unwrap_or(""),
                ref_ns = %denial.ref_namespace,
                ref_name = %denial.name,
                "edgion.io/dns-resolver-ref: cross-namespace reference to httpdns LinkSys has no ReferenceGrant; \
                 stripping annotation (data-plane DNS resolution falls back to the next layer)"
            );
            dns_resolver_guard::strip_dns_resolver_ref(&mut svc.metadata.annotations);
        }
        ProcessResult::Continue(svc)
    }

    async fn on_change(&self, svc: &Service, ctx: &HandlerContext) {
        self.requeue_dependent_routes(svc, "updated", ctx).await;
    }

    async fn on_delete(&self, svc: &Service, ctx: &HandlerContext) {
        self.requeue_dependent_routes(svc, "deleted", ctx).await;
    }
}

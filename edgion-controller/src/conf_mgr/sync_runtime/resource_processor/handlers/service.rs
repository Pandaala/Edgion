//! Service Handler
//!
//! Handles Service resources with cascading requeue for dependent routes.
//! When a Service is created/updated/deleted, routes that reference it as
//! a backend are requeued so their ResolvedRefs status is re-evaluated.

use k8s_openapi::api::core::v1::Service;

use crate::conf_mgr::sync_runtime::resource_processor::handlers::dns_resolver_guard;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ResourceRef};
use edgion_resources::ResourceKind;

/// Service handler
pub struct ServiceHandler;

impl ServiceHandler {
    pub fn new() -> Self {
        Self
    }

    async fn requeue_dependents(&self, svc: &Service, event: &str, ctx: &HandlerContext) {
        let svc_ns = svc.metadata.namespace.as_deref().unwrap_or("default");
        let svc_name = svc.metadata.name.as_deref().unwrap_or("");
        let service_key = format!("{}/{}", svc_ns, svc_name);

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
            event = %event,
            "Service changed, requeuing dependent resources"
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
        let owner = ResourceRef::new(
            ResourceKind::Service,
            svc.metadata.namespace.clone(),
            svc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&owner);

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
            &owner,
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

    async fn on_delete(&self, svc: &Service, ctx: &HandlerContext) {
        let owner = ResourceRef::new(
            ResourceKind::Service,
            svc.metadata.namespace.clone(),
            svc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&owner);
    }

    async fn after_publish(&self, svc: &Service, ctx: &HandlerContext) {
        self.requeue_dependents(svc, "updated", ctx).await;
    }

    async fn after_unpublish(&self, svc: &Service, ctx: &HandlerContext) {
        self.requeue_dependents(svc, "deleted", ctx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::service_ref::mirror_service_key;
    use crate::conf_mgr::sync_runtime::resource_processor::{ResourceProcessor, ResourceRef};
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::resources::EdgionPlugins;
    use edgion_resources::ResourceKind;
    use kube::api::ObjectMeta;
    use std::sync::Arc;

    #[tokio::test]
    async fn dns_resolver_grant_dependencies_replace_and_clear_on_delete() {
        let runtime = Arc::new(ControllerRuntime::default());
        let grant: edgion_resources::resources::ReferenceGrant = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "ReferenceGrant",
            "metadata": {"namespace": "dns", "name": "allow-service"},
            "spec": {
                "from": [{"group": "", "kind": "Service", "namespace": "app"}],
                "to": [{"group": "edgion.io", "kind": "LinkSys", "name": "aliyun"}]
            }
        }))
        .unwrap();
        runtime
            .ref_grant_store
            .upsert("dns/allow-service".to_string(), Arc::new(grant));
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        let handler = ServiceHandler::new();
        let mut service = Service {
            metadata: ObjectMeta {
                namespace: Some("app".to_string()),
                name: Some("backend".to_string()),
                annotations: Some(std::collections::BTreeMap::from([(
                    edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF.to_string(),
                    "dns/aliyun".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };
        let expected_owner = ResourceRef::new(ResourceKind::Service, Some("app".to_string()), "backend".to_string());

        let ProcessResult::Continue(parsed) = handler.parse(service.clone(), &ctx).await else {
            panic!("permitted Service must continue");
        };
        assert!(parsed
            .metadata
            .annotations
            .as_ref()
            .is_some_and(|annotations| annotations
                .contains_key(edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF)));
        assert_eq!(runtime.cross_ns_ref_mgr.get_refs("dns"), vec![expected_owner.clone()]);

        service.metadata.annotations = None;
        let ProcessResult::Continue(_) = handler.parse(service.clone(), &ctx).await else {
            panic!("replacement Service must continue");
        };
        assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());

        service.metadata.annotations = Some(std::collections::BTreeMap::from([(
            edgion_resources::constants::annotations::edgion::DNS_RESOLVER_REF.to_string(),
            "dns/aliyun".to_string(),
        )]));
        let ProcessResult::Continue(restored) = handler.parse(service, &ctx).await else {
            panic!("restored Service must continue");
        };
        assert_eq!(runtime.cross_ns_ref_mgr.get_refs("dns"), vec![expected_owner]);
        handler.on_delete(&restored, &ctx).await;
        assert!(runtime.cross_ns_ref_mgr.get_refs("dns").is_empty());
    }

    #[tokio::test]
    async fn mirror_dependents_requeue_only_after_service_visibility_changes() {
        let runtime = Arc::new(ControllerRuntime::default());
        let service_processor = Arc::new(ResourceProcessor::<Service>::new(
            "Service",
            16,
            Arc::new(ServiceHandler::new()),
            runtime.clone(),
        ));
        runtime.processor_registry.register(service_processor.clone());
        let plugin_processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::new()),
            runtime.clone(),
        ));
        runtime.processor_registry.register(plugin_processor.clone());

        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "mirrors".to_string(),
        );
        runtime
            .service_ref_mgr
            .add_ref(mirror_service_key("default", "shadow"), owner);
        let service = Service {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("shadow".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        let handler = ServiceHandler::new();

        handler.on_change(&service, &ctx).await;
        assert!(plugin_processor.workqueue().is_empty());
        service_processor.save(service.clone(), None);
        assert!(super::super::route_utils::service_exists("default", "shadow", &runtime));
        handler.after_publish(&service, &ctx).await;
        let published = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            plugin_processor.workqueue().dequeue(),
        )
        .await
        .expect("publish should requeue the mirror consumer")
        .expect("plugin workqueue should remain open");
        assert_eq!(published.key, "default/mirrors");

        handler.on_delete(&service, &ctx).await;
        assert!(plugin_processor.workqueue().is_empty());
        service_processor.remove("default/shadow", None);
        assert!(!super::super::route_utils::service_exists(
            "default", "shadow", &runtime
        ));
        handler.after_unpublish(&service, &ctx).await;
        let unpublished = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            plugin_processor.workqueue().dequeue(),
        )
        .await
        .expect("unpublish should requeue the mirror consumer")
        .expect("plugin workqueue should remain open");
        assert_eq!(unpublished.key, "default/mirrors");
    }
}

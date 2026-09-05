//! ConfigMap Handler
//!
//! Minimal handler for ConfigMap resources. Populates the ConfigMapStore
//! (via ControllerRuntime) so BackendTLSPolicy can resolve `caCertificateRefs`
//! with `kind: ConfigMap`.

use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1::ConfigMap;

use crate::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key,
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    HandlerContext, ProcessResult, ProcessorHandler,
};

pub struct ConfigMapHandler;

impl ConfigMapHandler {
    pub fn new() -> Self {
        Self
    }

    async fn trigger_cascading_requeue(&self, cm_key: &str, event: &str, ctx: &HandlerContext) {
        let mut refs = ctx.secret_ref_manager().get_refs(cm_key);
        if let Some((namespace, name)) = cm_key.split_once('/') {
            refs.extend(
                ctx.runtime()
                    .frontend_ca_ref_manager
                    .get_refs(&FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, namespace, name).index_key()),
            );
        }
        refs.sort_by_key(|resource| resource.key());
        refs.dedup();
        if !refs.is_empty() {
            tracing::info!(
                configmap_key = %cm_key,
                ref_count = refs.len(),
                event = %event,
                "Triggering cascading requeue for resources referencing ConfigMap"
            );
        }
        for resource_ref in refs {
            let key = match &resource_ref.namespace {
                Some(ns) => format!("{}/{}", ns, resource_ref.name),
                None => resource_ref.name.clone(),
            };
            ctx.requeue(resource_ref.kind.as_str(), key).await;
        }
    }
}

impl Default for ConfigMapHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<ConfigMap> for ConfigMapHandler {
    async fn parse(&self, cm: ConfigMap, ctx: &HandlerContext) -> ProcessResult<ConfigMap> {
        let cm_key = format_secret_key(
            cm.metadata.namespace.as_ref(),
            cm.metadata.name.as_deref().unwrap_or(""),
        );

        let mut upsert = HashMap::new();
        upsert.insert(cm_key.clone(), cm.clone());
        ctx.runtime().configmap_store.update(upsert, &HashSet::new());

        tracing::debug!(configmap_key = %cm_key, "ConfigMap parsed and added to ConfigMapStore");
        ProcessResult::Continue(cm)
    }

    async fn on_change(&self, cm: &ConfigMap, ctx: &HandlerContext) {
        let cm_key = format_secret_key(
            cm.metadata.namespace.as_ref(),
            cm.metadata.name.as_deref().unwrap_or(""),
        );
        self.trigger_cascading_requeue(&cm_key, "updated", ctx).await;
    }

    async fn on_delete(&self, cm: &ConfigMap, ctx: &HandlerContext) {
        let cm_key = format_secret_key(
            cm.metadata.namespace.as_ref(),
            cm.metadata.name.as_deref().unwrap_or(""),
        );

        let mut remove = HashSet::new();
        remove.insert(cm_key.clone());
        ctx.runtime().configmap_store.update(HashMap::new(), &remove);

        self.trigger_cascading_requeue(&cm_key, "deleted", ctx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::{
        frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
        HandlerContext, ProcessorHandler, ResourceProcessor, ResourceRef,
    };
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::Gateway;
    use edgion_resources::ResourceKind;
    use kube::api::ObjectMeta;
    use std::sync::Arc;

    struct GatewayNoop;

    #[async_trait::async_trait]
    impl ProcessorHandler<Gateway> for GatewayNoop {}

    #[tokio::test]
    async fn configmap_delete_then_recreate_preserves_typed_edge_and_requeues_twice() {
        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            100,
            Arc::new(GatewayNoop),
            runtime.clone(),
        ));
        runtime.processor_registry.register(processor.clone());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        let dependent = ResourceRef::new(
            ResourceKind::Gateway,
            Some("default".to_string()),
            "gateway".to_string(),
        );
        let source = FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "default", "same-name");
        runtime.frontend_ca_ref_manager.add_ref(source.index_key(), dependent);
        let configmap = ConfigMap {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("same-name".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        ConfigMapHandler::new().on_delete(&configmap, &ctx).await;
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.key, "default/gateway");
        processor.workqueue().done(&first.key);
        assert_eq!(runtime.frontend_ca_ref_manager.get_refs(&source.index_key()).len(), 1);

        ConfigMapHandler::new().on_change(&configmap, &ctx).await;
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.key, "default/gateway");
        processor.workqueue().done(&second.key);
    }
}

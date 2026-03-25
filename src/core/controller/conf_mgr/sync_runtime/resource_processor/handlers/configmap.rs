//! ConfigMap Handler
//!
//! Minimal handler for ConfigMap resources. Populates the global ConfigMapStore
//! so BackendTLSPolicy can resolve `caCertificateRefs` with `kind: ConfigMap`.

use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1::ConfigMap;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, update_configmaps, HandlerContext, ProcessResult, ProcessorHandler,
};

pub struct ConfigMapHandler;

impl ConfigMapHandler {
    pub fn new() -> Self {
        Self
    }

    async fn trigger_cascading_requeue(&self, cm_key: &str, event: &str, ctx: &HandlerContext) {
        let refs = ctx.secret_ref_manager().get_refs(cm_key);
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
    async fn parse(&self, cm: ConfigMap, _ctx: &HandlerContext) -> ProcessResult<ConfigMap> {
        let cm_key = format_secret_key(
            cm.metadata.namespace.as_ref(),
            cm.metadata.name.as_deref().unwrap_or(""),
        );

        let mut upsert = HashMap::new();
        upsert.insert(cm_key.clone(), cm.clone());
        update_configmaps(upsert, &HashSet::new());

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

    fn on_delete(&self, cm: &ConfigMap, ctx: &HandlerContext) {
        let cm_key = format_secret_key(
            cm.metadata.namespace.as_ref(),
            cm.metadata.name.as_deref().unwrap_or(""),
        );

        let mut remove = HashSet::new();
        remove.insert(cm_key.clone());
        update_configmaps(HashMap::new(), &remove);

        self.trigger_cascading_requeue(&cm_key, "deleted", ctx);
    }
}

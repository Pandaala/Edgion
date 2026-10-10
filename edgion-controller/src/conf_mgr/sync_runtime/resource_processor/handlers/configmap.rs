//! ConfigMap Handler
//!
//! Minimal handler for ConfigMap resources. Populates the ConfigMapStore
//! (via ControllerRuntime) so TLS material consumers can resolve
//! `caCertificateRefs` with `kind: ConfigMap`.

use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1::ConfigMap;

use crate::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key,
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    HandlerContext, ProcessResult, ProcessorHandler, CONFIG_MAP_REF_KEY_PREFIX,
};

pub struct ConfigMapHandler;

impl ConfigMapHandler {
    pub fn new() -> Self {
        Self
    }

    async fn trigger_cascading_requeue(&self, cm_key: &str, source: &ConfigMap, event: &str, ctx: &HandlerContext) {
        let mut refs = ctx
            .secret_ref_manager()
            .get_refs(&format!("{CONFIG_MAP_REF_KEY_PREFIX}{cm_key}"));
        if let Some(name) = source.metadata.name.as_deref() {
            let namespace = source.metadata.namespace.as_deref().unwrap_or("default");
            if source.metadata.namespace.is_none() {
                // Preserve all raw-key consumers; the semantic alias is limited to actual Gateway/BTP/EBTP material consumers.
                refs.extend(
                    ctx.secret_ref_manager()
                        .get_refs(&format!("{CONFIG_MAP_REF_KEY_PREFIX}default/{name}"))
                        .into_iter()
                        .filter(|resource| {
                            matches!(
                                resource.kind,
                                edgion_resources::ResourceKind::Gateway
                                    | edgion_resources::ResourceKind::BackendTLSPolicy
                                    | edgion_resources::ResourceKind::EdgionBackendTrafficPolicy
                            )
                        }),
                );
            }
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
        self.trigger_cascading_requeue(&cm_key, cm, "updated", ctx).await;
    }

    async fn on_delete(&self, cm: &ConfigMap, ctx: &HandlerContext) {
        let cm_key = format_secret_key(
            cm.metadata.namespace.as_ref(),
            cm.metadata.name.as_deref().unwrap_or(""),
        );

        let mut remove = HashSet::new();
        remove.insert(cm_key.clone());
        ctx.runtime().configmap_store.update(HashMap::new(), &remove);

        self.trigger_cascading_requeue(&cm_key, cm, "deleted", ctx).await;
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/configmap_test.rs"]
mod tests;

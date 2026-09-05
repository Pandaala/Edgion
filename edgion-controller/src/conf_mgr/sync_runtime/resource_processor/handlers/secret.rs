//! Secret Handler
//!
//! Handles Secret resources with:
//! - Global SecretStore updates for TLS callback access
//! - Cascading requeue for dependent resources (EdgionTls, Gateway)

use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap, HashSet};

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;

use crate::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key,
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use edgion_resources::ResourceKind;

/// Secret handler
///
/// Features:
/// - parse: Normalize stringData→data, update SecretStore, validate OIDC secret shape
/// - on_change: Trigger cascading requeue for dependent resources
/// - on_delete: Remove from SecretStore + trigger cascading requeue
///
/// Accumulates secrets during init LIST phase, then performs an authoritative
/// `replace_all_secrets` on init_done to purge stale entries from previous sessions.
pub struct SecretHandler {
    init_accumulator: RwLock<Option<HashMap<String, Secret>>>,
}

impl SecretHandler {
    pub fn new() -> Self {
        Self {
            init_accumulator: RwLock::new(Some(HashMap::new())),
        }
    }

    /// Trigger cascading requeue for all resources that depend on this secret
    async fn trigger_cascading_requeue(&self, secret_key: &str, event: &str, ctx: &HandlerContext) {
        let mut refs = ctx.secret_ref_manager().get_refs(secret_key);
        if let Some((namespace, name)) = secret_key.split_once('/') {
            refs.extend(
                ctx.runtime()
                    .frontend_ca_ref_manager
                    .get_refs(&FrontendCaSourceKey::new(FrontendCaSourceKind::Secret, namespace, name).index_key()),
            );
        }
        refs.sort_by_key(ResourceRef::key);
        refs.dedup();

        if !refs.is_empty() {
            tracing::info!(
                secret_key = %secret_key,
                ref_count = refs.len(),
                event = %event,
                "Triggering cascading requeue for referencing resources"
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

    fn has_non_empty_secret_field(secret: &Secret, keys: &[&str]) -> bool {
        if let Some(data) = &secret.data {
            for key in keys {
                if let Some(value) = data.get(*key) {
                    if !value.0.is_empty() {
                        return true;
                    }
                }
            }
        }
        if let Some(string_data) = &secret.string_data {
            for key in keys {
                if let Some(value) = string_data.get(*key) {
                    if !value.trim().is_empty() {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn validate_oidc_secret_shape(&self, secret: &Secret, secret_key: &str, ctx: &HandlerContext) {
        let refs = ctx.secret_ref_manager().get_refs(secret_key);
        if refs.is_empty() {
            return;
        }
        if !refs.iter().any(|r| r.kind == ResourceKind::EdgionPlugins) {
            return;
        }

        let has_oidc_field = Self::has_non_empty_secret_field(
            secret,
            &[
                "clientSecret",
                "client_secret",
                "sessionSecret",
                "session_secret",
                "secret",
            ],
        );
        if !has_oidc_field {
            tracing::warn!(
                secret_key = %secret_key,
                "Referenced Secret has no OIDC-compatible key; expected one of clientSecret/client_secret/sessionSecret/session_secret/secret"
            );
        }
    }
}

impl Default for SecretHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Merge `stringData` into `data` following Kubernetes semantics.
///
/// - `stringData` values overwrite any `data` entry with the same key.
/// - `stringData` is cleared after merging, so the in-memory Secret has a
///   single authoritative `data` map.
/// - No-op when both maps are absent or empty.
fn normalize_string_data(secret: &mut Secret) {
    let Some(string_data) = secret.string_data.take() else {
        return;
    };
    if string_data.is_empty() {
        return;
    }
    let data = secret.data.get_or_insert_with(BTreeMap::new);
    for (k, v) in string_data {
        data.insert(k, ByteString(v.into_bytes()));
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<Secret> for SecretHandler {
    async fn parse(&self, mut secret: Secret, ctx: &HandlerContext) -> ProcessResult<Secret> {
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        // Normalize `stringData` into `data` so downstream consumers
        // (SecretMaterialResolver, TLS callback, plugin pipelines) see a single
        // `data` map regardless of how the Secret was authored.
        //
        // Matches Kubernetes server-side semantics: stringData values are
        // copied into data (UTF-8 -> bytes) and overwrite any pre-existing
        // entries with the same key.
        normalize_string_data(&mut secret);

        // During init phase, accumulate for authoritative replace_all at init_done.
        {
            let mut acc = self.init_accumulator.write();
            if let Some(ref mut map) = *acc {
                map.insert(secret_key.clone(), secret.clone());
            }
        }

        let mut upsert = HashMap::new();
        upsert.insert(secret_key.clone(), secret.clone());
        ctx.runtime().secret_store.update(upsert, &HashSet::new());

        tracing::debug!(
            secret_key = %secret_key,
            "Secret parsed and added to SecretStore"
        );
        self.validate_oidc_secret_shape(&secret, &secret_key, ctx);

        ProcessResult::Continue(secret)
    }

    fn on_init_done(&self, _ctx: &HandlerContext) {
        let accumulated = self.init_accumulator.write().take();
        if let Some(secrets) = accumulated {
            let count = secrets.len();
            _ctx.runtime().secret_store.replace_all(secrets);
            tracing::info!(count = count, "SecretStore authoritative replace_all on init_done");
        }
    }

    async fn on_change(&self, secret: &Secret, ctx: &HandlerContext) {
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        self.validate_oidc_secret_shape(secret, &secret_key, ctx);

        // SecretStore is already updated in parse(); only trigger cascading requeue here.
        self.trigger_cascading_requeue(&secret_key, "updated", ctx).await;
    }

    async fn on_delete(&self, secret: &Secret, ctx: &HandlerContext) {
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        // 1. Update global SecretStore (delete)
        let mut remove = HashSet::new();
        remove.insert(secret_key.clone());
        ctx.runtime().secret_store.update(HashMap::new(), &remove);

        // 2. Trigger cascading requeue (so dependent resources know Secret is deleted)
        self.trigger_cascading_requeue(&secret_key, "deleted", ctx).await;

        // Do not clear source-side dependency edges. A delete followed by a
        // recreate may coalesce before the owner reconciles; both events must
        // continue to requeue every registered owner. Only owner reconcile may
        // clear-and-rebuild its reverse edges.
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_string_data, SecretHandler};
    use crate::conf_mgr::sync_runtime::resource_processor::{
        frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
        HandlerContext, ProcessorHandler, ResourceProcessor, ResourceRef,
    };
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::EdgionPlugins;
    use edgion_resources::ResourceKind;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    struct PluginsNoop;

    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for PluginsNoop {}

    async fn assert_plugins_requeued(processor: &ResourceProcessor<EdgionPlugins>) {
        let item = tokio::time::timeout(Duration::from_secs(1), processor.workqueue().dequeue())
            .await
            .expect("dependent plugin should be requeued")
            .expect("work item should exist");
        assert_eq!(item.key, "default/policy");
        processor.workqueue().done(&item.key);
    }

    #[test]
    fn test_has_non_empty_secret_field_reads_data() {
        let mut secret = Secret::default();
        let mut data = BTreeMap::new();
        data.insert("clientSecret".to_string(), ByteString(b"abc".to_vec()));
        secret.data = Some(data);

        assert!(SecretHandler::has_non_empty_secret_field(
            &secret,
            &["clientSecret", "client_secret"]
        ));
    }

    #[test]
    fn test_has_non_empty_secret_field_reads_string_data() {
        let mut secret = Secret::default();
        let mut string_data = BTreeMap::new();
        string_data.insert("session_secret".to_string(), "v".to_string());
        secret.string_data = Some(string_data);

        assert!(SecretHandler::has_non_empty_secret_field(
            &secret,
            &["sessionSecret", "session_secret"]
        ));
    }

    #[test]
    fn test_has_non_empty_secret_field_false_when_missing() {
        let secret = Secret::default();
        assert!(!SecretHandler::has_non_empty_secret_field(
            &secret,
            &["clientSecret", "client_secret", "secret"]
        ));
    }

    #[test]
    fn normalize_string_data_copies_into_data_and_clears() {
        let mut secret = Secret::default();
        let mut sd = BTreeMap::new();
        sd.insert("password".to_string(), "hunter2".to_string());
        sd.insert("username".to_string(), "alice".to_string());
        secret.string_data = Some(sd);

        normalize_string_data(&mut secret);

        assert!(secret.string_data.is_none(), "stringData must be cleared");
        let data = secret.data.expect("data must be present");
        assert_eq!(data.get("password").unwrap().0, b"hunter2".to_vec());
        assert_eq!(data.get("username").unwrap().0, b"alice".to_vec());
    }

    #[test]
    fn normalize_string_data_overrides_existing_data_entry() {
        let mut secret = Secret::default();
        let mut data = BTreeMap::new();
        data.insert("password".to_string(), ByteString(b"old".to_vec()));
        secret.data = Some(data);
        let mut sd = BTreeMap::new();
        sd.insert("password".to_string(), "new".to_string());
        secret.string_data = Some(sd);

        normalize_string_data(&mut secret);

        let data = secret.data.unwrap();
        assert_eq!(
            data.get("password").unwrap().0,
            b"new".to_vec(),
            "stringData must win over data for duplicate keys"
        );
    }

    #[test]
    fn normalize_string_data_noop_when_absent() {
        let mut secret = Secret::default();
        let mut data = BTreeMap::new();
        data.insert("key".to_string(), ByteString(b"val".to_vec()));
        secret.data = Some(data);

        normalize_string_data(&mut secret);

        assert!(secret.string_data.is_none());
        assert_eq!(secret.data.unwrap().get("key").unwrap().0, b"val".to_vec());
    }

    #[tokio::test]
    async fn secret_apply_and_delete_requeue_nested_request_access_policy_dependents() {
        let runtime = Arc::new(ControllerRuntime::default());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            100,
            Arc::new(PluginsNoop),
            runtime.clone(),
        ));
        runtime.processor_registry.register(processor.clone());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        let secret = Secret {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("origin-token".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let dependent = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "policy".to_string(),
        );

        runtime
            .secret_ref_manager
            .add_ref("default/origin-token".to_string(), dependent.clone());
        SecretHandler::new().on_change(&secret, &ctx).await;
        assert_plugins_requeued(&processor).await;

        runtime
            .secret_ref_manager
            .add_ref("default/origin-token".to_string(), dependent);
        SecretHandler::new().on_delete(&secret, &ctx).await;
        assert_plugins_requeued(&processor).await;
        assert_eq!(runtime.secret_ref_manager.get_refs("default/origin-token").len(), 1);

        SecretHandler::new().on_change(&secret, &ctx).await;
        assert_plugins_requeued(&processor).await;
    }

    #[tokio::test]
    async fn frontend_ca_source_delete_preserves_typed_edges_and_kind_isolation() {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        let dependent = ResourceRef::new(
            ResourceKind::Gateway,
            Some("default".to_string()),
            "gateway".to_string(),
        );
        let secret_key = FrontendCaSourceKey::new(FrontendCaSourceKind::Secret, "default", "same-name");
        let configmap_key = FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "default", "same-name");
        runtime
            .frontend_ca_ref_manager
            .add_ref(secret_key.index_key(), dependent.clone());
        runtime
            .frontend_ca_ref_manager
            .add_ref(configmap_key.index_key(), dependent);

        let secret = Secret {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("same-name".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        SecretHandler::new().on_delete(&secret, &ctx).await;

        assert_eq!(
            runtime.frontend_ca_ref_manager.get_refs(&secret_key.index_key()).len(),
            1
        );
        assert_eq!(
            runtime
                .frontend_ca_ref_manager
                .get_refs(&configmap_key.index_key())
                .len(),
            1
        );
    }
}

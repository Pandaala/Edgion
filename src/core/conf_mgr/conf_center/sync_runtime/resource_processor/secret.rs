//! Secret Processor
//!
//! Handles Secret resources with:
//! - Global SecretStore updates for TLS callback access
//! - Cascading requeue for dependent resources (EdgionTls, Gateway)

use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1::Secret;

use super::{format_secret_key, ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::update_secrets;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};

/// Secret processor
///
/// Features:
/// - parse: No special processing
/// - save: Insert/update cache + update global SecretStore
/// - on_change: Update global SecretStore + trigger cascading requeue via RequeueRegistry
/// - on_delete: Update global SecretStore (delete) + trigger cascading requeue
pub struct SecretProcessor;

impl SecretProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SecretProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<Secret> for SecretProcessor {
    fn kind(&self) -> &'static str {
        "Secret"
    }

    fn parse(&self, secret: Secret, _ctx: &ProcessContext) -> ProcessResult<Secret> {
        ProcessResult::Continue(secret)
    }

    fn save(&self, cs: &ConfigServer, secret: Secret) {
        cs.secrets.apply_change(ResourceChange::EventUpdate, secret);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.secrets.get_by_key(key) {
            cs.secrets.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn on_change(&self, secret: &Secret, ctx: &ProcessContext) {
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        // 1. Update global SecretStore (for TLS callback access)
        let mut add_or_update = HashMap::new();
        add_or_update.insert(secret_key.clone(), secret.clone());
        update_secrets(add_or_update, HashMap::new(), &HashSet::new());

        // 2. Trigger cascading requeue for dependent resources
        let refs = ctx.secret_ref_manager().get_refs(&secret_key);

        if !refs.is_empty() {
            tracing::info!(
                secret_key = %secret_key,
                ref_count = refs.len(),
                "Secret updated, triggering cascading requeue for referencing resources"
            );
        }

        for resource_ref in refs {
            let key = match &resource_ref.namespace {
                Some(ns) => format!("{}/{}", ns, resource_ref.name),
                None => resource_ref.name.clone(),
            };

            // Use RequeueRegistry to enqueue key to the corresponding resource's workqueue
            ctx.requeue_registry()
                .enqueue(resource_ref.kind.as_str(), key);
        }
    }

    fn on_delete(&self, secret: &Secret, ctx: &ProcessContext) {
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        // 1. Update global SecretStore (delete)
        let mut remove = HashSet::new();
        remove.insert(secret_key.clone());
        update_secrets(HashMap::new(), HashMap::new(), &remove);

        // 2. Trigger cascading requeue (so dependent resources know Secret is deleted)
        let refs = ctx.secret_ref_manager().get_refs(&secret_key);

        if !refs.is_empty() {
            tracing::info!(
                secret_key = %secret_key,
                ref_count = refs.len(),
                "Secret deleted, triggering cascading requeue for referencing resources"
            );
        }

        for resource_ref in refs {
            let key = match &resource_ref.namespace {
                Some(ns) => format!("{}/{}", ns, resource_ref.name),
                None => resource_ref.name.clone(),
            };

            ctx.requeue_registry()
                .enqueue(resource_ref.kind.as_str(), key);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<Secret> {
        cs.secrets.get_by_key(key)
    }
}

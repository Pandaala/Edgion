//! Secret Handler
//!
//! Handles Secret resources with:
//! - Global SecretStore updates for TLS callback access
//! - Cascading requeue for dependent resources (EdgionTls, Gateway)

use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1::Secret;

use crate::core::conf_mgr_new::sync_runtime::resource_processor::{
    format_secret_key, update_secrets, HandlerContext, ProcessResult, ProcessorHandler,
};

/// Secret handler
///
/// Features:
/// - parse: No special processing
/// - on_change: Update global SecretStore + trigger cascading requeue via ProcessorRegistry
/// - on_delete: Update global SecretStore (delete) + trigger cascading requeue
pub struct SecretHandler;

impl SecretHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SecretHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<Secret> for SecretHandler {
    fn parse(&self, secret: Secret, _ctx: &HandlerContext) -> ProcessResult<Secret> {
        ProcessResult::Continue(secret)
    }

    fn on_change(&self, secret: &Secret, ctx: &HandlerContext) {
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

            // Use ProcessorRegistry to enqueue key to the corresponding resource's workqueue
            ctx.requeue(resource_ref.kind.as_str(), key);
        }
    }

    fn on_delete(&self, secret: &Secret, ctx: &HandlerContext) {
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

            ctx.requeue(resource_ref.kind.as_str(), key);
        }
    }
}

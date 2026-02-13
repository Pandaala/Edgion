//! Secret Handler
//!
//! Handles Secret resources with:
//! - Global SecretStore updates for TLS callback access
//! - Cascading requeue for dependent resources (EdgionTls, Gateway)

use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1::Secret;

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, update_secrets, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::types::ResourceKind;

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

    /// Trigger cascading requeue for all resources that depend on this secret
    fn trigger_cascading_requeue(&self, secret_key: &str, event: &str, ctx: &HandlerContext) {
        let refs = ctx.secret_ref_manager().get_refs(secret_key);

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
            ctx.requeue(resource_ref.kind.as_str(), key);
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

impl ProcessorHandler<Secret> for SecretHandler {
    fn parse(&self, secret: Secret, ctx: &HandlerContext) -> ProcessResult<Secret> {
        // Update global SecretStore during parse
        // This ensures SecretStore is populated during Init phase
        // (on_change is not called during Init)
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        let mut upsert = HashMap::new();
        upsert.insert(secret_key.clone(), secret.clone());
        update_secrets(upsert, &HashSet::new());

        tracing::debug!(
            secret_key = %secret_key,
            "Secret parsed and added to SecretStore"
        );
        self.validate_oidc_secret_shape(&secret, &secret_key, ctx);

        ProcessResult::Continue(secret)
    }

    fn on_change(&self, secret: &Secret, ctx: &HandlerContext) {
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        self.validate_oidc_secret_shape(secret, &secret_key, ctx);

        // SecretStore is already updated in parse(), so we only trigger cascading requeue here
        // Trigger cascading requeue for dependent resources
        self.trigger_cascading_requeue(&secret_key, "updated", ctx);
    }

    fn on_delete(&self, secret: &Secret, ctx: &HandlerContext) {
        let secret_key = format_secret_key(
            secret.metadata.namespace.as_ref(),
            secret.metadata.name.as_deref().unwrap_or(""),
        );

        // 1. Update global SecretStore (delete)
        let mut remove = HashSet::new();
        remove.insert(secret_key.clone());
        update_secrets(HashMap::new(), &remove);

        // 2. Trigger cascading requeue (so dependent resources know Secret is deleted)
        self.trigger_cascading_requeue(&secret_key, "deleted", ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::SecretHandler;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

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
}

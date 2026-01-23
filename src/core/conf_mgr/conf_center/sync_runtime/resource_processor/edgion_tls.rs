//! EdgionTls Processor
//!
//! Handles EdgionTls resources with:
//! - Validation (warnings for missing secrets, parent_refs, etc.)
//! - Server Secret reference resolution (secret_ref -> spec.secret)
//! - CA Secret reference resolution for mTLS (client_auth.ca_secret_ref -> client_auth.ca_secret)
//! - SecretRefManager registration

use super::validation::check_edgion_tls;
use super::{find_secret, format_secret_key, ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server::{ConfigServer, ResourceRef};
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::EdgionTls;
use crate::types::ResourceKind;

/// EdgionTls processor
///
/// Features:
/// - validate: Check for parent_refs and secret existence
/// - parse: Parse secret_ref -> fill spec.secret
/// - parse: Parse client_auth.ca_secret_ref -> fill client_auth.ca_secret
/// - parse: Register Secret references to SecretRefManager
/// - on_delete: Clear SecretRefManager references
pub struct EdgionTlsProcessor;

impl EdgionTlsProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionTlsProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProcessor<EdgionTls> for EdgionTlsProcessor {
    fn kind(&self) -> &'static str {
        "EdgionTls"
    }

    fn validate(&self, tls: &EdgionTls, ctx: &ProcessContext) -> Vec<String> {
        let result = check_edgion_tls(ctx.config_server, tls);
        result.warnings
    }

    fn parse(&self, mut tls: EdgionTls, ctx: &ProcessContext) -> ProcessResult<EdgionTls> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        let secret_list = ctx.list_secrets();

        // 1. Resolve server Secret
        let secret_ns = tls
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(tls.metadata.namespace.as_ref());
        let secret_key = format_secret_key(secret_ns, &tls.spec.secret_ref.name);

        // Register reference relationship
        ctx.secret_ref_manager()
            .add_ref(secret_key.clone(), resource_ref.clone());

        // Try to resolve Secret
        if let Some(secret) = find_secret(&secret_list, secret_ns, &tls.spec.secret_ref.name) {
            tls.spec.secret = Some(secret.clone());
            tracing::debug!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Secret resolved and filled into EdgionTls"
            );
        } else {
            tracing::warn!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Secret not found, EdgionTls will be sent without Secret data"
            );
        }

        // 2. Resolve CA Secret (if mTLS is configured)
        if let Some(ref mut client_auth) = tls.spec.client_auth {
            if let Some(ref ca_secret_ref) = client_auth.ca_secret_ref {
                let ca_ns = ca_secret_ref.namespace.as_ref().or(tls.metadata.namespace.as_ref());
                let ca_secret_key = format_secret_key(ca_ns, &ca_secret_ref.name);

                // Register CA Secret reference
                ctx.secret_ref_manager()
                    .add_ref(ca_secret_key.clone(), resource_ref.clone());

                // Try to resolve CA Secret
                if let Some(ca_secret) = find_secret(&secret_list, ca_ns, &ca_secret_ref.name) {
                    client_auth.ca_secret = Some(ca_secret.clone());
                    tracing::debug!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "CA Secret resolved and filled into EdgionTls.client_auth"
                    );
                } else {
                    tracing::warn!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "CA Secret not found, mTLS will not work"
                    );
                }
            }
        }

        ProcessResult::Continue(tls)
    }

    fn on_delete(&self, tls: &EdgionTls, ctx: &ProcessContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        tracing::debug!(
            edgion_tls = %resource_ref.key(),
            "Cleared secret references on EdgionTls delete"
        );
    }

    fn save(&self, cs: &ConfigServer, tls: EdgionTls) {
        cs.edgion_tls.apply_change(ResourceChange::EventUpdate, tls);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.edgion_tls.get_by_key(key) {
            cs.edgion_tls.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<EdgionTls> {
        cs.edgion_tls.get_by_key(key)
    }
}

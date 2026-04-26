//! JWT credential resolution.

use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, PendingReason, ResolveOutcome,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::edgion_plugins::plugin_configs::ResolvedJwtCredential;
use crate::types::resources::edgion_plugins::EdgionPlugin;

/// Resolve JWT credentials from Secrets and register references to SecretRefManager.
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    // Process request plugins
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::JwtAuth(ref mut config) = entry.plugin {
                // Resolve single secret_ref
                if let Some(ref sor) = config.secret_ref.clone() {
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", sor) {
                        ResolveOutcome::Ok(secret) => {
                            if let Some(data) = &secret.data {
                                let mut cred = ResolvedJwtCredential::default();
                                // Try "secret" key for HS*
                                if let Some(secret_bytes) = data.get("secret") {
                                    cred.secret = Some(STANDARD.encode(&secret_bytes.0));
                                }
                                // Try "publicKey" key for RS*/ES*
                                if let Some(pk_bytes) = data.get("publicKey") {
                                    cred.public_key = String::from_utf8(pk_bytes.0.clone()).ok();
                                }
                                if cred.secret.is_some() || cred.public_key.is_some() {
                                    config.resolved_credential = Some(cred);
                                    tracing::debug!(
                                        edgion_plugins = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        "JwtAuth: Secret resolved and credential filled"
                                    );
                                }
                            }
                        }
                        ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "JwtAuth: Secret not found yet, will be reprocessed when Secret arrives"
                            );
                        }
                        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                            ref from_ns,
                            ref to_ns,
                            ref secret_name,
                        }) => {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                from_ns = %from_ns,
                                to_ns = %to_ns,
                                secret_name = %secret_name,
                                "JwtAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                            );
                        }
                        ResolveOutcome::Invalid(_) => {}
                    }
                }

                // Resolve multiple secret_refs
                if let Some(ref secret_refs) = config.secret_refs.clone() {
                    let mut resolved = HashMap::new();
                    for sor in secret_refs {
                        let secret_key =
                            format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                        match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", sor) {
                            ResolveOutcome::Ok(secret) => {
                                if let Some(data) = &secret.data {
                                    let mut cred = ResolvedJwtCredential::default();
                                    // Get "key" identifier
                                    if let Some(key_bytes) = data.get("key") {
                                        cred.key = String::from_utf8(key_bytes.0.clone()).ok();
                                    }
                                    // Get "secret" for HS*
                                    if let Some(secret_bytes) = data.get("secret") {
                                        cred.secret = Some(STANDARD.encode(&secret_bytes.0));
                                    }
                                    // Get "publicKey" for RS*/ES*
                                    if let Some(pk_bytes) = data.get("publicKey") {
                                        cred.public_key = String::from_utf8(pk_bytes.0.clone()).ok();
                                    }
                                    if let Some(ref key) = cred.key {
                                        resolved.insert(key.clone(), cred);
                                    }
                                }
                            }
                            ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                                tracing::info!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "JwtAuth: Secret not found yet, will be reprocessed when Secret arrives"
                                );
                                continue;
                            }
                            ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                                ref from_ns,
                                ref to_ns,
                                ref secret_name,
                            }) => {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    from_ns = %from_ns,
                                    to_ns = %to_ns,
                                    secret_name = %secret_name,
                                    "JwtAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                                );
                                continue;
                            }
                            ResolveOutcome::Invalid(_) => continue,
                        }
                    }
                    if !resolved.is_empty() {
                        config.resolved_credentials = Some(resolved);
                    }
                }
            }
        }
    }
}

//! JWE decrypt credential resolution.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, PendingReason, ResolveOutcome,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::common::SecretReference;
use crate::types::resources::edgion_plugins::plugin_configs::ResolvedJweCredential;
use crate::types::resources::edgion_plugins::EdgionPlugin;

/// Resolve JWE decrypt credentials from Secrets and register references.
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::JweDecrypt(ref mut config) = entry.plugin {
                config.resolved_credential = None;

                let Some(sor) = config.secret_ref.clone() else {
                    continue;
                };
                let sref = SecretReference {
                    name: sor.name.clone(),
                    namespace: sor.namespace.clone(),
                };

                let secret_key =
                    format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sref) {
                    ResolveOutcome::Ok(s) => s,
                    ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "JweDecrypt: Secret not found yet, will be reprocessed when Secret arrives"
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
                            "JweDecrypt: cross-namespace Secret reference denied (ReferenceGrant missing)"
                        );
                        continue;
                    }
                    ResolveOutcome::Invalid(_) => continue,
                };

                let mut resolved = ResolvedJweCredential::default();

                if let Some(data) = &secret.data {
                    if let Some(secret_bytes) = data.get("secret") {
                        resolved.secret = Some(STANDARD.encode(&secret_bytes.0));
                    }
                }

                if resolved.secret.is_none() {
                    if let Some(string_data) = &secret.string_data {
                        if let Some(secret_str) = string_data.get("secret") {
                            resolved.secret = Some(STANDARD.encode(secret_str.as_bytes()));
                        }
                    }
                }

                if resolved.secret.is_some() {
                    config.resolved_credential = Some(resolved);
                    tracing::debug!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        "JweDecrypt: Secret resolved and credential filled"
                    );
                }
            }
        }
    }
}

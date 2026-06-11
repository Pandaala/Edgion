//! JWE decrypt credential resolution.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, FetchOutcome, PendingReason,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::ResolvedJweCredential;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

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
                let secret_key =
                    format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                    FetchOutcome::Ok(s) => s,
                    FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "JweDecrypt: Secret not found yet, will be reprocessed when Secret arrives"
                        );
                        continue;
                    }
                    FetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
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
                };

                let mut resolved = ResolvedJweCredential::default();

                // SecretHandler::parse normalizes stringData into data, so the
                // secret_store always exposes the value through `secret.data`.
                if let Some(data) = &secret.data {
                    if let Some(secret_bytes) = data.get("secret") {
                        resolved.secret = Some(STANDARD.encode(&secret_bytes.0));
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

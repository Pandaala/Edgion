//! JWE decrypt credential resolution.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResolvedRefsError, ResourceRef,
};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::ResolvedJweCredential;
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, ResolvedJweCredentialError};

/// Resolve JWE decrypt credentials from Secrets and register references.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            let enabled = entry.is_enabled();
            if let EdgionPlugin::JweDecrypt(ref mut config) = entry.plugin {
                config.resolved_credential = None;
                if !enabled {
                    continue;
                }

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

                config.resolved_credential = Some(resolved);
                if config
                    .resolved_credential
                    .as_ref()
                    .is_some_and(|value| value.secret.is_some())
                {
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

/// Report present-but-unusable JWE Secret material without re-reading a Secret
/// or registering dependencies during status derivation. Missing and denied
/// references leave `resolved_credential` absent and are covered by the shared
/// dependency classifier.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn status_errors(
    ep: &EdgionPlugins,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    let Some(entries) = ep.spec.request_plugins.as_ref() else {
        return errors;
    };
    for entry in entries {
        if !entry.is_enabled() {
            continue;
        }
        let EdgionPlugin::JweDecrypt(config) = &entry.plugin else {
            continue;
        };
        if config.resolved_credential.is_none() {
            continue;
        }
        match config.resolved_key_material() {
            Ok(_) => {}
            Err(ResolvedJweCredentialError::Missing) => {
                errors.push(ResolvedRefsError::MissingKeys { keys: vec!["secret"] });
            }
            Err(error) => errors.push(ResolvedRefsError::DecodeError {
                key: "secret",
                detail: error.status_detail(),
            }),
        }
    }
    errors
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/jwe_test.rs"]
mod tests;

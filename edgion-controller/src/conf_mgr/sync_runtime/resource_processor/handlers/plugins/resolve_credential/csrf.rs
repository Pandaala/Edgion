//! CSRF signing-key resolution.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

use super::read_secret_utf8;

/// Resolve CSRF signing keys from Secrets and register references.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::Csrf(ref mut config) = entry.plugin {
                config.resolved_key = None;

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
                            "Csrf: Secret not found yet, will be reprocessed when Secret arrives"
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
                            "Csrf: cross-namespace Secret reference denied (ReferenceGrant missing)"
                        );
                        continue;
                    }
                };

                if let Some(key) = read_secret_utf8(&secret, &["key"]) {
                    config.resolved_key = Some(key.to_owned());
                    tracing::debug!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        "Csrf: signing key resolved"
                    );
                }
            }
        }
    }
}

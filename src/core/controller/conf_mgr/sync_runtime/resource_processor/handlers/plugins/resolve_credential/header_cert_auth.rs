//! HeaderCertAuth CA secret resolution.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, PendingReason, ResolveOutcome,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::common::SecretReference;
use crate::types::resources::edgion_plugins::plugin_configs::CertSourceMode;
use crate::types::resources::edgion_plugins::EdgionPlugin;

/// Resolve HeaderCertAuth CA secrets and register secret references.
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::HeaderCertAuth(ref mut config) = entry.plugin {
                config.resolved_ca_secrets = None;

                if config.mode != CertSourceMode::Header {
                    continue;
                }
                if config.ca_secret_refs.is_empty() {
                    continue;
                }

                let mut resolved = Vec::new();
                for sor in &config.ca_secret_refs {
                    let sref = SecretReference {
                        name: sor.name.clone(),
                        namespace: sor.namespace.clone(),
                    };
                    let secret_key = format_secret_key(
                        sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()),
                        &sor.name,
                    );
                    let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sref) {
                        ResolveOutcome::Ok(s) => s,
                        ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "HeaderCertAuth: CA secret not found yet, will be reprocessed when Secret arrives"
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
                                "HeaderCertAuth: cross-namespace CA Secret reference denied (ReferenceGrant missing)"
                            );
                            continue;
                        }
                        ResolveOutcome::Invalid(_) => continue,
                    };

                    let has_ca = secret.data.as_ref().is_some_and(|data| data.contains_key("ca.crt"))
                        || secret
                            .string_data
                            .as_ref()
                            .is_some_and(|data| data.contains_key("ca.crt"));
                    if !has_ca {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "HeaderCertAuth: Secret missing ca.crt"
                        );
                        continue;
                    }

                    resolved.push(secret);
                }

                if !resolved.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        ca_secret_count = resolved.len(),
                        "HeaderCertAuth: Resolved CA secrets"
                    );
                    config.resolved_ca_secrets = Some(resolved);
                }
            }
        }
    }
}

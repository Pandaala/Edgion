//! BasicAuth credential resolution.

use std::collections::HashMap;

use super::read_secret_utf8;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, PendingReason, ResolveOutcome,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::common::SecretReference;
use crate::types::resources::edgion_plugins::EdgionPlugin;

/// Resolve BasicAuth users from Secrets and register references to SecretRefManager.
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::BasicAuth(ref mut config) = entry.plugin {
                config.resolved_users = None;

                let Some(secret_refs) = config.secret_refs.clone() else {
                    continue;
                };
                if secret_refs.is_empty() {
                    continue;
                }

                let mut resolved = HashMap::new();
                for sor in &secret_refs {
                    let sref: SecretReference = sor.into();
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sref) {
                        ResolveOutcome::Ok(s) => s,
                        ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "BasicAuth: Secret not found yet, will be reprocessed when Secret arrives"
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
                                "BasicAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                            );
                            continue;
                        }
                        ResolveOutcome::Invalid(_) => continue,
                    };

                    let Some(username) = read_secret_utf8(&secret, &["username"]) else {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "BasicAuth: Secret missing username"
                        );
                        continue;
                    };
                    let Some(password) = read_secret_utf8(&secret, &["password"]) else {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "BasicAuth: Secret missing password"
                        );
                        continue;
                    };

                    if resolved.contains_key(&username) {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            username = %username,
                            "BasicAuth: duplicate username found, skipping"
                        );
                        continue;
                    }
                    resolved.insert(username, password);
                }

                if !resolved.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        user_count = resolved.len(),
                        "BasicAuth: Resolved users from Secrets"
                    );
                    config.resolved_users = Some(resolved);
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        "BasicAuth: No users resolved from Secrets"
                    );
                }
            }
        }
    }
}

//! OpenID Connect credential resolution.

use super::read_secret_utf8;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve OpenID Connect secrets and register references to SecretRefManager.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::OpenidConnect(ref mut config) = entry.plugin {
                // Reset runtime-resolved fields first to avoid stale values on update.
                config.resolved_client_secret = None;
                config.resolved_session_secret = None;

                if let Some(sor) = config.client_secret_ref.clone() {
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(secret) => {
                            let resolved = read_secret_utf8(&secret, &["clientSecret", "client_secret", "secret"]);
                            if let Some(value) = resolved {
                                config.resolved_client_secret = Some(value);
                                tracing::debug!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: client secret resolved"
                                );
                            } else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: Secret missing client secret key (clientSecret/client_secret/secret)"
                                );
                            }
                        }
                        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "OpenidConnect: client secret not found yet, will be reprocessed when Secret arrives"
                            );
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
                                "OpenidConnect: cross-namespace client secret reference denied (ReferenceGrant missing)"
                            );
                        }
                    }
                }

                if let Some(sor) = config.session_secret_ref.clone() {
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(secret) => {
                            let resolved = read_secret_utf8(&secret, &["sessionSecret", "session_secret", "secret"]);
                            if let Some(value) = resolved {
                                config.resolved_session_secret = Some(value);
                                tracing::debug!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: session secret resolved"
                                );
                            } else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "OpenidConnect: Secret missing session secret key (sessionSecret/session_secret/secret)"
                                );
                            }
                        }
                        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "OpenidConnect: session secret not found yet, will be reprocessed when Secret arrives"
                            );
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
                                "OpenidConnect: cross-namespace session secret reference denied (ReferenceGrant missing)"
                            );
                        }
                    }
                }
            }
        }
    }
}

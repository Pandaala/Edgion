//! LdapAuth credential resolution.
//!
//! Resolves the outbound TLS secret refs on each `LdapAuth` plugin
//! (`outbound.caSecretRef` → `resolved_ca_secret` and
//! `outbound.clientSecretRef` → `resolved_client_secret`) and registers
//! the references with `SecretRefManager` (via `fetch_secret`) so that a
//! future Secret change triggers a re-process of the owning `EdgionPlugins`
//! resource.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve LdapAuth TLS secrets and register references with SecretRefManager.
///
/// LdapAuth only appears in `request_plugins` (it implements `RequestFilter` only),
/// so a single-list iteration is exhaustive.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let ep_ns = ep.metadata.namespace.clone();

    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::LdapAuth(ref mut config) = entry.plugin {
                // Reset runtime-resolved fields to avoid stale values on update.
                config.resolved_ca_secret = None;
                config.resolved_client_secret = None;

                // TLS CA secret.
                if let Some(sor) = config.tls.ca_secret_ref.clone() {
                    let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_ns.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(secret) => {
                            config.resolved_ca_secret = Some(secret);
                            tracing::debug!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "LdapAuth: CA secret resolved"
                            );
                        }
                        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "LdapAuth: CA secret not found yet, will be reprocessed when Secret arrives"
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
                                "LdapAuth: cross-namespace CA secret reference denied (ReferenceGrant missing)"
                            );
                        }
                    }
                }

                // TLS client-identity secret (mTLS).
                if let Some(sor) = config.tls.client_secret_ref.clone() {
                    let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_ns.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(secret) => {
                            config.resolved_client_secret = Some(secret);
                            tracing::debug!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "LdapAuth: client identity secret resolved"
                            );
                        }
                        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "LdapAuth: client identity secret not found yet, will be reprocessed when Secret arrives"
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
                                "LdapAuth: cross-namespace client identity secret reference denied (ReferenceGrant missing)"
                            );
                        }
                    }
                }
            }
        }
    }
}

//! OpenID Connect credential resolution.

use super::read_secret_utf8;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, resolve_ca_certificate_refs, CaFetchStrategy, FetchOutcome, PendingReason,
};
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
                config.resolved_oidc_client_secret = None;
                config.resolved_session_secret = None;
                config.resolved_ca_certificates = None;
                config.resolved_client_certificate = None;

                // OAuth2 client secret (UTF-8 string value).
                if let Some(sor) = config.client_secret_ref.clone() {
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(secret) => {
                            let resolved = read_secret_utf8(&secret, &["clientSecret", "client_secret", "secret"]);
                            if let Some(value) = resolved {
                                config.resolved_oidc_client_secret = Some(value);
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

                // TLS CA certificate refs (multi-bundle).
                let ca_refs = config
                    .tls
                    .validation
                    .as_ref()
                    .and_then(|v| v.ca_certificate_refs.as_deref())
                    .unwrap_or_default();
                let resolved_cas = resolve_ca_certificate_refs(
                    ctx,
                    resource_ref,
                    ep.metadata.namespace.as_deref(),
                    ca_refs,
                    CaFetchStrategy::GrantGated {
                        group: "edgion.io",
                        kind: "EdgionPlugins",
                    },
                );
                config.resolved_ca_certificates = (!resolved_cas.is_empty()).then_some(resolved_cas);

                // TLS client-identity secret (mTLS; whole Secret object).
                if let Some(sor) = config.tls.client_certificate_ref.clone() {
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(secret) => {
                            config.resolved_client_certificate = Some(secret);
                            tracing::debug!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "OpenidConnect: client identity secret resolved"
                            );
                        }
                        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "OpenidConnect: client identity secret not found yet, will be reprocessed when Secret arrives"
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
                                "OpenidConnect: cross-namespace client identity secret reference denied (ReferenceGrant missing)"
                            );
                        }
                    }
                }
            }
        }
    }
}

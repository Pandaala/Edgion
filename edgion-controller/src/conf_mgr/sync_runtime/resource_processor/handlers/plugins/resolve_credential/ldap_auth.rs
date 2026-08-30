//! LdapAuth credential resolution.
//!
//! Resolves the outbound TLS refs on each `LdapAuth` plugin:
//! `tls.validation.caCertificateRefs` → `resolved_ca_certificates` and
//! `tls.clientCertificateRef` → `resolved_client_certificate`.
//! References are registered with `SecretRefManager` so that a future Secret
//! change triggers a re-process of the owning `EdgionPlugins` resource.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, resolve_ca_certificate_refs, CaFetchStrategy, FetchOutcome, PendingReason,
};
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
                config.resolved_ca_certificates = None;
                config.resolved_client_certificate = None;

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
                    ep_ns.as_deref(),
                    ca_refs,
                    CaFetchStrategy::GrantGated {
                        group: "edgion.io",
                        kind: "EdgionPlugins",
                    },
                );
                config.resolved_ca_certificates = (!resolved_cas.is_empty()).then_some(resolved_cas);

                // TLS client-identity secret (mTLS).
                if let Some(sor) = config.tls.client_certificate_ref.clone() {
                    let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_ns.as_ref()), &sor.name);
                    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(secret) => {
                            config.resolved_client_certificate = Some(secret);
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

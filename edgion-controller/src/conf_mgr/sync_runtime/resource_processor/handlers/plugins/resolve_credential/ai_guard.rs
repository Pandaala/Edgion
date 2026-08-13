//! AiGuard evaluator outbound request and TLS Secret resolution.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, resolve_ca_certificate_refs, resolve_request_secrets, CaFetchStrategy, FetchOutcome, PendingReason,
};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::common::ResolvedSecrets;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::ResourceKind;

/// Resolve request-template `${secretRef}` values and the managed HTTP TLS
/// CA/client-identity references for every request-stage `AiGuard` entry that
/// configures an external evaluator.
///
/// An `AiGuard` without an evaluator never dials outbound, so it resolves
/// nothing and is skipped entirely.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) async fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let ep_ns = ep.metadata.namespace.clone();
    let Some(entries) = ep.spec.request_plugins.as_mut() else {
        return;
    };

    for entry in entries {
        let EdgionPlugin::AiGuard(guard) = &mut entry.plugin else {
            continue;
        };
        let Some(config) = guard.evaluator.as_mut() else {
            continue;
        };

        config.resolved_secrets = None;
        if let Some(tls) = config.conn.tls.as_mut() {
            tls.resolved_ca_certificates = None;
            tls.resolved_client_certificate = None;

            let ca_refs = tls
                .outbound
                .validation
                .as_ref()
                .and_then(|validation| validation.ca_certificate_refs.as_deref())
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
            tls.resolved_ca_certificates = (!resolved_cas.is_empty()).then_some(resolved_cas);

            if let Some(secret_ref) = tls.outbound.client_certificate_ref.clone() {
                let secret_key = format_secret_key(secret_ref.namespace.as_ref().or(ep_ns.as_ref()), &secret_ref.name);
                match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &secret_ref) {
                    FetchOutcome::Ok(secret) => {
                        tls.resolved_client_certificate = Some(secret);
                        tracing::debug!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "AiGuard: evaluator client identity secret resolved"
                        );
                    }
                    FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                        tracing::info!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "AiGuard: evaluator client identity secret not found yet, will be reprocessed when Secret arrives"
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
                            "AiGuard: cross-namespace evaluator client identity secret reference denied (ReferenceGrant missing)"
                        );
                    }
                }
            }
        }

        let names = config.request.collect_secret_ref_names();
        if names.is_empty() {
            continue;
        }
        let (resolved, _errors) = resolve_request_secrets(ctx, resource_ref, ResourceKind::EdgionPlugins, &names).await;
        config.resolved_secrets = (!resolved.is_empty()).then_some(ResolvedSecrets(resolved));
    }
}

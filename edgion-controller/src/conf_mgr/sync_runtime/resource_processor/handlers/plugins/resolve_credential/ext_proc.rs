//! ExtProc credential resolution.
//!
//! Resolves the outbound TLS refs on each `ExtProc` plugin:
//! `grpcService.tls.validation.caCertificateRefs` → `resolved_ca_certificates` and
//! `grpcService.tls.clientCertificateRef` → `resolved_client_certificate`.
//! References are registered with `SecretRefManager` so that a future Secret
//! change triggers a re-process of the owning `EdgionPlugins` resource.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, resolve_ca_certificate_refs, CaFetchStrategy, FetchOutcome, PendingReason,
};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve ExtProc TLS secrets and register references with SecretRefManager.
///
/// ExtProc can be configured in any plugin stage (request and every upstream
/// response stage). The TLS material refs must be resolved wherever an ExtProc
/// entry appears — a response-stage ExtProc over TLS needs its
/// `validation.caCertificateRefs`/`clientCertificateRef` resolved just like the
/// request stage, otherwise the gateway fails closed with `OutboundTlsCaMissing`.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    // Own the namespace up front so the four stage lists (fields of `ep.spec`)
    // can each be borrowed mutably below without aliasing `ep.metadata`.
    let ep_ns = ep.metadata.namespace.clone();

    macro_rules! resolve_stage {
        ($stage:expr) => {
            if let Some(ref mut entries) = $stage {
                for entry in entries.iter_mut() {
                    resolve_ext_proc_plugin(&mut entry.plugin, ep_ns.as_ref(), resource_ref, ctx);
                }
            }
        };
    }

    resolve_stage!(ep.spec.request_plugins);
    resolve_stage!(ep.spec.upstream_response_filter_plugins);
    resolve_stage!(ep.spec.upstream_response_body_filter_plugins);
    resolve_stage!(ep.spec.upstream_response_plugins);
}

/// Resolve the TLS secret refs of a single plugin entry if it is an ExtProc.
/// No-op for any other plugin type.
fn resolve_ext_proc_plugin(
    plugin: &mut EdgionPlugin,
    ep_ns: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let EdgionPlugin::ExtProc(ref mut config) = plugin else {
        return;
    };

    // Reset runtime-resolved fields first to avoid stale values on update.
    config.grpc_service.tls.resolved_ca_certificates = None;
    config.grpc_service.tls.resolved_client_certificate = None;

    // TLS CA certificate refs (multi-bundle).
    let ca_refs = config
        .grpc_service
        .tls
        .outbound
        .validation
        .as_ref()
        .and_then(|v| v.ca_certificate_refs.as_deref())
        .unwrap_or_default();
    let resolved_cas = resolve_ca_certificate_refs(
        ctx,
        resource_ref,
        ep_ns.map(String::as_str),
        ca_refs,
        CaFetchStrategy::GrantGated {
            group: "edgion.io",
            kind: "EdgionPlugins",
        },
    );
    config.grpc_service.tls.resolved_ca_certificates = (!resolved_cas.is_empty()).then_some(resolved_cas);

    // TLS client-identity secret (mTLS; whole Secret object).
    if let Some(sor) = config.grpc_service.tls.outbound.client_certificate_ref.clone() {
        let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_ns), &sor.name);
        match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
            FetchOutcome::Ok(secret) => {
                config.grpc_service.tls.resolved_client_certificate = Some(secret);
                tracing::debug!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    "ExtProc: client identity secret resolved"
                );
            }
            FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                tracing::info!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    "ExtProc: client identity secret not found yet, will be reprocessed when Secret arrives"
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
                    "ExtProc: cross-namespace client identity secret reference denied (ReferenceGrant missing)"
                );
            }
        }
    }
}

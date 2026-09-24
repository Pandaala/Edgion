//! Wasm plugin credential resolution.
//!
//! Resolves the `source.fetch` refs on each `Wasm` plugin:
//! `fetch.tls.validation.caCertificateRefs` → `resolved_ca_certificates`,
//! `fetch.tls.clientCertificateRef` → `resolved_client_certificate`, and
//! `fetch.authHeaderSecretRef` → `resolved_auth_header` (the value the
//! gateway sends as the `Authorization` header when fetching `source.url`
//! bytecode from a private artifact repository). Also resolves
//! `source.oci.pullSecretRef` → `resolved_pull_secret` (the raw
//! dockerconfigjson document used to authenticate an OCI registry pull).
//!
//! References are registered with `SecretRefManager` (inside `fetch_secret` /
//! `resolve_ca_certificate_refs`) so a Secret change re-processes the owning
//! `EdgionPlugins` resource.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, resolve_ca_certificate_refs, CaFetchStrategy, FetchOutcome, PendingReason,
};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve Wasm `source.fetch` secrets in every stage list (a Wasm entry can
/// appear at the request stage and the upstream-response stages alike).
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let ep_ns = ep.metadata.namespace.clone();

    macro_rules! resolve_stage {
        ($stage:expr) => {
            if let Some(ref mut entries) = $stage {
                for entry in entries.iter_mut() {
                    resolve_wasm_plugin(&mut entry.plugin, ep_ns.as_ref(), resource_ref, ctx);
                }
            }
        };
    }

    resolve_stage!(ep.spec.request_plugins);
    resolve_stage!(ep.spec.upstream_response_filter_plugins);
    resolve_stage!(ep.spec.upstream_response_body_filter_plugins);
    resolve_stage!(ep.spec.upstream_response_plugins);
}

/// Resolve the fetch refs of a single plugin entry if it is a Wasm plugin
/// with a `source.fetch` block. No-op otherwise.
fn resolve_wasm_plugin(
    plugin: &mut EdgionPlugin,
    ep_ns: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let EdgionPlugin::Wasm(ref mut config) = plugin else {
        return;
    };

    resolve_oci_pull_secret(config, ep_ns, resource_ref, ctx);

    let Some(ref mut fetch) = config.source.fetch else {
        return;
    };

    // Reset runtime-resolved fields first to avoid stale values on update.
    fetch.resolved_auth_header = None;

    if let Some(ref mut tls) = fetch.tls {
        tls.resolved_ca_certificates = None;
        tls.resolved_client_certificate = None;

        let ca_refs = tls
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
        tls.resolved_ca_certificates = (!resolved_cas.is_empty()).then_some(resolved_cas);

        if let Some(sor) = tls.outbound.client_certificate_ref.clone() {
            let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_ns), &sor.name);
            match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                FetchOutcome::Ok(secret) => {
                    tls.resolved_client_certificate = Some(secret);
                }
                FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        "Wasm fetch: client identity secret not found yet, will reprocess when the Secret arrives"
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
                        "Wasm fetch: cross-namespace client identity secret reference denied (ReferenceGrant missing)"
                    );
                }
            }
        }
    }

    if let Some(auth) = fetch.auth_header_secret_ref.clone() {
        let secret_key = format_secret_key(auth.secret.namespace.as_ref().or(ep_ns), &auth.secret.name);
        match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &auth.secret) {
            FetchOutcome::Ok(secret) => match super::read_secret_utf8(&secret, &[auth.key.as_str()]) {
                Some(value) => {
                    fetch.resolved_auth_header = Some(value.to_owned());
                }
                None => {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        auth_key = %auth.key,
                        "Wasm fetch: auth header Secret found but key is missing or empty"
                    );
                }
            },
            FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                tracing::info!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    "Wasm fetch: auth header secret not found yet, will reprocess when the Secret arrives"
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
                    "Wasm fetch: cross-namespace auth header secret reference denied (ReferenceGrant missing)"
                );
            }
        }
    }
}

/// Resolve `source.oci.pullSecretRef` into `resolved_pull_secret` (the raw
/// dockerconfigjson document; the gateway matches the registry host at pull
/// time). An unresolvable reference leaves the field `None`, which the
/// gateway treats as a load failure — never a silent anonymous pull.
fn resolve_oci_pull_secret(
    config: &mut edgion_resources::resources::edgion_plugins::plugin_configs::WasmConfig,
    ep_ns: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Some(ref mut oci) = config.source.oci else {
        return;
    };
    // Reset first to avoid stale values on update.
    oci.resolved_pull_secret = None;
    let Some(sor) = oci.pull_secret_ref.clone() else {
        return;
    };
    let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_ns), &sor.name);
    match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
        FetchOutcome::Ok(secret) => match extract_dockerconfigjson(&secret) {
            Ok(json) => oci.resolved_pull_secret = Some(json),
            Err(reason) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    reason = %reason,
                    "Wasm oci: pull secret found but invalid"
                );
            }
        },
        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
            tracing::info!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                "Wasm oci: pull secret not found yet, will reprocess when the Secret arrives"
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
                "Wasm oci: cross-namespace pull secret reference denied (ReferenceGrant missing)"
            );
        }
    }
}

/// Validate the Secret shape for an image pull secret: type
/// `kubernetes.io/dockerconfigjson` with a UTF-8 `.dockerconfigjson` key.
fn extract_dockerconfigjson(secret: &k8s_openapi::api::core::v1::Secret) -> Result<String, String> {
    if secret.type_.as_deref() != Some("kubernetes.io/dockerconfigjson") {
        return Err(format!(
            "Secret type must be kubernetes.io/dockerconfigjson, got '{}'",
            secret.type_.as_deref().unwrap_or("")
        ));
    }
    super::read_secret_utf8(secret, &[".dockerconfigjson"])
        .map(str::to_owned)
        .ok_or_else(|| "Secret key .dockerconfigjson is missing or not UTF-8".to_string())
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/wasm_test.rs"]
mod tests;

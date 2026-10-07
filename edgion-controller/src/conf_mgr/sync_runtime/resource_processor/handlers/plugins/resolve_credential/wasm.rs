//! Wasm fetch and registry credentials resolved from actual owning fields.

use super::read_secret_utf8;
use crate::conf_mgr::sync_runtime::resource_processor::credential::ca_refs::resolve_bound_tls;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    prepared: &PreparedPluginReferences,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Ok(mut bindings) = prepared.bind_mut(ep) else {
        return;
    };
    for entry in bindings.request_entries() {
        resolve_entry(entry, owner, ctx);
    }
    for entry in bindings.response_filter_entries() {
        resolve_entry(entry, owner, ctx);
    }
    for entry in bindings.response_body_filter_entries() {
        resolve_entry(entry, owner, ctx);
    }
    for entry in bindings.response_entries() {
        resolve_entry(entry, owner, ctx);
    }
}

fn resolve_entry(mut entry: BoundPluginEntryMut<'_>, owner: &ResourceRef, ctx: &HandlerContext) {
    let EdgionPlugin::Wasm(config) = entry.config_mut() else {
        return;
    };
    if let Some(oci) = config.source.oci.as_mut() {
        oci.resolved_pull_secret = None;
    }
    if let Some(fetch) = config.source.fetch.as_mut() {
        fetch.resolved_auth_header = None;
        if let Some(tls) = fetch.tls.as_mut() {
            tls.resolved_ca_certificates = None;
            tls.resolved_client_certificate = None;
        }
    }
    let (auth, pull, tls) = {
        let EdgionPlugin::Wasm(config) = entry.config() else {
            return;
        };
        let Ok(references) = entry.references() else { return };
        let auth = references.wasm_auth_header_secret().and_then(|token| {
            let key = &config.source.fetch.as_ref()?.auth_header_secret_ref.as_ref()?.key;
            match fetch_bound_secret(ctx, owner, token) {
                BoundFetchOutcome::Ok(authorized) => read_secret_utf8(&authorized.secret, &[key]).map(str::to_owned),
                BoundFetchOutcome::Pending(_) | BoundFetchOutcome::Invalid(_) => None,
            }
        });
        let pull = references
            .wasm_pull_secret()
            .and_then(|token| match fetch_bound_secret(ctx, owner, token) {
                BoundFetchOutcome::Ok(authorized) => extract_dockerconfigjson(&authorized.secret).ok(),
                BoundFetchOutcome::Pending(_) | BoundFetchOutcome::Invalid(_) => None,
            });
        (
            auth,
            pull,
            references.tls().map(|tls| resolve_bound_tls(ctx, owner, tls)),
        )
    };
    if let EdgionPlugin::Wasm(config) = entry.config_mut() {
        if let Some(oci) = config.source.oci.as_mut() {
            oci.resolved_pull_secret = pull;
        }
        if let Some(fetch) = config.source.fetch.as_mut() {
            fetch.resolved_auth_header = auth;
            if let (Some(tls), Some(material)) = (fetch.tls.as_mut(), tls) {
                tls.resolved_ca_certificates = material.ca_certificates;
                tls.resolved_client_certificate = material.client_certificate;
            }
        }
    }
}

/// Read-only status follows the same owning fields, activity and material checks.
/// OCI parsing stays shallow: registry-host and credential matching belong to
/// the Gateway's pull implementation and continue failing closed there.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn status_errors(
    ep: &EdgionPlugins,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) -> Vec<crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError> {
    use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::{
        bound_secret_status_error, bound_secret_status_result,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError;
    let prepared = PreparedPluginReferences::from_plugins(ep);
    let bindings = match prepared.bind(ep) {
        Ok(bindings) => bindings,
        Err(error) => return vec![ResolvedRefsError::InvalidReference { error }],
    };
    let mut errors = Vec::new();
    for entry in bindings
        .request_entries()
        .chain(bindings.response_filter_entries())
        .chain(bindings.response_body_filter_entries())
        .chain(bindings.response_entries())
    {
        let EdgionPlugin::Wasm(config) = entry.config() else {
            continue;
        };
        let references = match entry.references() {
            Ok(references) => references,
            Err(error) => {
                errors.push(ResolvedRefsError::InvalidReference { error });
                continue;
            }
        };
        if let Some(token) = references.wasm_auth_header_secret() {
            match bound_secret_status_result(ctx, owner, token) {
                Err(error) => errors.push(error),
                Ok(authorized) => {
                    if let Some(auth) = config
                        .source
                        .fetch
                        .as_ref()
                        .and_then(|fetch| fetch.auth_header_secret_ref.as_ref())
                    {
                        if read_secret_utf8(&authorized.secret, &[&auth.key]).is_none() {
                            errors.push(ResolvedRefsError::DecodeError {
                                key: "authHeaderSecretRef",
                                detail: format!("Secret data key '{}' is missing, empty, or not UTF-8", auth.key),
                            });
                        }
                    }
                }
            }
        }
        if let Some(token) = references.wasm_pull_secret() {
            match bound_secret_status_result(ctx, owner, token) {
                Err(error) => errors.push(error),
                Ok(authorized) => {
                    if authorized.secret.type_.as_deref() != Some("kubernetes.io/dockerconfigjson") {
                        errors.push(ResolvedRefsError::InvalidSecretType {
                            got: authorized.secret.type_.clone().unwrap_or_default(),
                            allowed: &["kubernetes.io/dockerconfigjson"],
                        });
                    } else if read_secret_utf8(&authorized.secret, &[".dockerconfigjson"]).is_none() {
                        errors.push(ResolvedRefsError::DecodeError {
                            key: ".dockerconfigjson",
                            detail: "Secret key .dockerconfigjson is missing, empty, or not UTF-8".to_string(),
                        });
                    }
                }
            }
        }
        if let Some(tls) = references.tls() {
            for token in tls.ca_certificates() {
                errors.extend(
                    crate::conf_mgr::sync_runtime::resource_processor::credential::ca_refs::bound_ca_status_error(
                        ctx, owner, token,
                    ),
                );
            }
            if let Some(token) = tls.client_certificate() {
                errors.extend(bound_secret_status_error(ctx, owner, token));
            }
        }
    }
    errors
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

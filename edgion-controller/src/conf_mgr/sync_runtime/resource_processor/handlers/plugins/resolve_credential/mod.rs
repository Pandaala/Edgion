//! Per-plugin-type Secret resolution.
//!
//! Each submodule defines a single `resolve(ep, resource_ref, ctx)` function
//! that reads Kubernetes Secret data and populates the plugin's resolved
//! fields. `edgion_plugins.rs::parse` dispatches to them in sequence.
//!
//! The re-exports below expose each resolver under a disambiguated name
//! (`resolve_basic_auth`, `resolve_jwt`, ...) so call sites remain greppable
//! even though each submodule exports a locally simple `resolve`.

mod basic_auth;
mod credential_injector;
mod csrf;
mod ext_proc;
mod header_cert_auth;
mod hmac;
mod jwe;
mod jwt;
mod key_auth;
mod ldap_auth;
mod managed_http;
mod openid_connect;
mod wasm;

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use basic_auth::resolve as resolve_basic_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use credential_injector::{
    resolve as resolve_credential_injector, status_errors as credential_injector_status_errors,
};
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use csrf::resolve as resolve_csrf;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use ext_proc::resolve as resolve_ext_proc;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use header_cert_auth::resolve as resolve_header_cert_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use hmac::{
    resolve as resolve_hmac, resolve_condition as resolve_condition_hmac,
};
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use jwe::{
    resolve as resolve_jwe, status_errors as jwe_status_errors,
};
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use jwt::resolve as resolve_jwt;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use key_auth::resolve as resolve_key_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use ldap_auth::resolve as resolve_ldap_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use managed_http::resolve as resolve_managed_http;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use openid_connect::resolve as resolve_openid_connect;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use wasm::{
    resolve as resolve_wasm, status_errors as wasm_status_errors,
};

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};

/// Report bound fields even when invalid identity prevented dependency creation.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn bound_credential_status_errors(
    ep: &edgion_resources::EdgionPlugins,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) -> Vec<crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError> {
    use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::bound_secret_status_error;
    use crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError;
    use edgion_resources::resources::edgion_plugins::EdgionPlugin;
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
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
        if !entry.is_enabled() {
            continue;
        }
        let references = match entry.references() {
            Ok(references) => references,
            Err(error) => {
                errors.push(ResolvedRefsError::InvalidReference { error });
                continue;
            }
        };
        match entry.config() {
            EdgionPlugin::BasicAuth(_)
            | EdgionPlugin::KeyAuth(_)
            | EdgionPlugin::JwtAuth(_)
            | EdgionPlugin::HmacAuth(_) => {
                for group in references.secret_groups() {
                    errors.extend(
                        group
                            .references()
                            .filter_map(|token| bound_secret_status_error(ctx, owner, token)),
                    );
                }
            }
            EdgionPlugin::Csrf(_) => {
                errors.extend(
                    references
                        .csrf_secret()
                        .and_then(|token| bound_secret_status_error(ctx, owner, token)),
                );
            }
            EdgionPlugin::JweDecrypt(_) => {
                errors.extend(
                    references
                        .jwe_secret()
                        .and_then(|token| bound_secret_status_error(ctx, owner, token)),
                );
            }
            EdgionPlugin::HeaderCertAuth(_) => {
                errors.extend(
                    references
                        .header_ca_secrets()
                        .filter_map(|token| bound_secret_status_error(ctx, owner, token)),
                );
            }
            EdgionPlugin::OpenidConnect(_) => {
                for token in references
                    .oidc_client_secret()
                    .into_iter()
                    .chain(references.oidc_session_secret())
                {
                    errors.extend(bound_secret_status_error(ctx, owner, token));
                }
            }
            EdgionPlugin::LdapAuth(_) => {
                errors.extend(
                    references
                        .ldap_bind_secret()
                        .and_then(|token| bound_secret_status_error(ctx, owner, token)),
                );
            }
            EdgionPlugin::ForwardAuth(_) | EdgionPlugin::Guardrail(_) | EdgionPlugin::AiGuard(_) => {
                errors.extend(super::super::super::credential::request_secret_errors_readonly(
                    ctx,
                    owner,
                    references.request_secrets(),
                ));
            }
            _ => {}
        }
        if matches!(
            entry.config(),
            EdgionPlugin::OpenidConnect(_)
                | EdgionPlugin::LdapAuth(_)
                | EdgionPlugin::ExtProc(_)
                | EdgionPlugin::ForwardAuth(_)
                | EdgionPlugin::Guardrail(_)
                | EdgionPlugin::AiGuard(_)
        ) {
            if let Some(tls) = references.tls() {
                for token in tls.ca_certificates() {
                    errors.extend(super::super::super::credential::ca_refs::bound_ca_status_error(
                        ctx, owner, token,
                    ));
                }
                if let Some(token) = tls.client_certificate() {
                    errors.extend(bound_secret_status_error(ctx, owner, token));
                }
            }
        }
    }
    errors
}

/// Read a UTF-8 value from a Secret's `data` map. Returns the first non-empty
/// match across the provided keys.
///
/// `SecretHandler::parse` normalizes `stringData` into `data` and clears
/// `stringData`, so Secrets coming from `secret_store` always have
/// `string_data == None`; only the `data` map needs to be inspected.
///
/// Visible to the whole `handlers` module so `edgion_plugins.rs`'s status
/// derivation (`wasm::status_errors`) can re-run the *exact* same usable-material
/// check the Wasm resolver used at `parse` time — sharing this one function is
/// what guarantees status and the resolver never disagree.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn read_secret_utf8<'a>(
    secret: &'a k8s_openapi::api::core::v1::Secret,
    keys: &[&str],
) -> Option<&'a str> {
    let data = secret.data.as_ref()?;
    for key in keys {
        if let Some(bytes) = data.get(*key) {
            if let Ok(value) = std::str::from_utf8(&bytes.0) {
                if !value.trim().is_empty() {
                    return Some(value);
                }
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/mod_test.rs"]
mod tests;

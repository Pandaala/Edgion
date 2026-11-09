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
            EdgionPlugin::OpenidConnect(config) => {
                // Usable material is checked only for secrets this
                // configuration may use; an unused reference still reports
                // fetch and authorization errors, as before.
                errors.extend(references.oidc_client_secret().and_then(|token| {
                    material_status_error(
                        ctx,
                        owner,
                        token,
                        config.may_use_client_secret().then_some(&OIDC_CLIENT_SECRET),
                    )
                }));
                errors.extend(references.oidc_session_secret().and_then(|token| {
                    material_status_error(
                        ctx,
                        owner,
                        token,
                        config.may_use_session_secret().then_some(&OIDC_SESSION_SECRET),
                    )
                }));
            }
            EdgionPlugin::LdapAuth(_) => {
                errors.extend(
                    references
                        .ldap_bind_secret()
                        .and_then(|token| material_status_error(ctx, owner, token, Some(&LDAP_BIND_PASSWORD))),
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

/// What makes a bound credential Secret usable: the resolver's accepted keys
/// (any one is enough), their description for `MissingKeys`, and a minimum
/// value length in bytes (0 = any non-blank value).
struct MaterialRule {
    field: &'static str,
    keys: &'static [&'static str],
    keys_description: &'static str,
    min_len: usize,
}

const LDAP_BIND_PASSWORD: MaterialRule = MaterialRule {
    field: "bindPasswordRef",
    keys: edgion_resources::resources::edgion_plugins::plugin_configs::LdapAuthConfig::BIND_PASSWORD_KEYS,
    keys_description: "password",
    min_len: 0,
};
const OIDC_CLIENT_SECRET: MaterialRule = MaterialRule {
    field: "clientSecretRef",
    keys: edgion_resources::resources::edgion_plugins::plugin_configs::OpenidConnectConfig::CLIENT_SECRET_KEYS,
    keys_description: "one of clientSecret, client_secret, secret",
    min_len: 0,
};
const OIDC_SESSION_SECRET: MaterialRule = MaterialRule {
    field: "sessionSecretRef",
    keys: edgion_resources::resources::edgion_plugins::plugin_configs::OpenidConnectConfig::SESSION_SECRET_KEYS,
    keys_description: "one of sessionSecret, session_secret, secret",
    min_len: edgion_resources::resources::edgion_plugins::plugin_configs::OpenidConnectConfig::MIN_SESSION_SECRET_LEN,
};

/// Status for one bound credential reference: fetch and authorization errors
/// always; with a `rule`, also unusable content, matching what the resolver
/// reads with [`read_secret_utf8`]: no accepted key → `MissingKeys`; keys
/// present but every value blank or not UTF-8, or shorter than the rule's
/// minimum → `DecodeError` naming the field. Details never contain Secret data.
fn material_status_error(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    token: edgion_resources::resources::refs::ReferenceToken<'_>,
    rule: Option<&MaterialRule>,
) -> Option<crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError> {
    use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::bound_secret_status_result;
    use crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError;
    let authorized = match bound_secret_status_result(ctx, owner, token) {
        Ok(authorized) => authorized,
        Err(error) => return Some(error),
    };
    let rule = rule?;
    let present = authorized
        .secret
        .data
        .as_ref()
        .is_some_and(|data| rule.keys.iter().any(|key| data.contains_key(*key)));
    if !present {
        return Some(ResolvedRefsError::MissingKeys {
            keys: vec![rule.keys_description],
        });
    }
    match read_secret_utf8(&authorized.secret, rule.keys) {
        None => Some(ResolvedRefsError::DecodeError {
            key: rule.field,
            detail: format!("Secret value under {} is blank or not UTF-8", rule.keys.join(" / ")),
        }),
        Some(value) if value.len() < rule.min_len => Some(ResolvedRefsError::DecodeError {
            key: rule.field,
            detail: format!("Secret value is shorter than {} bytes", rule.min_len),
        }),
        Some(_) => None,
    }
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

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
mod csrf;
mod forward_auth;
mod global_access_control;
mod header_cert_auth;
mod hmac;
mod jwe;
mod jwt;
mod key_auth;
mod openid_connect;

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use basic_auth::resolve as resolve_basic_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use csrf::resolve as resolve_csrf;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use forward_auth::resolve as resolve_forward_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use global_access_control::resolve as resolve_global_access_control;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use header_cert_auth::resolve as resolve_header_cert_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use hmac::resolve as resolve_hmac;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use jwe::resolve as resolve_jwe;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use jwt::resolve as resolve_jwt;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use key_auth::resolve as resolve_key_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use openid_connect::resolve as resolve_openid_connect;

/// Read a UTF-8 value from a Secret's `data` map. Returns the first non-empty
/// match across the provided keys.
///
/// `SecretHandler::parse` normalizes `stringData` into `data` and clears
/// `stringData`, so Secrets coming from `secret_store` always have
/// `string_data == None`; only the `data` map needs to be inspected.
pub(super) fn read_secret_utf8(secret: &k8s_openapi::api::core::v1::Secret, keys: &[&str]) -> Option<String> {
    let data = secret.data.as_ref()?;
    for key in keys {
        if let Some(bytes) = data.get(*key) {
            if let Ok(value) = String::from_utf8(bytes.0.clone()) {
                if !value.trim().is_empty() {
                    return Some(value);
                }
            }
        }
    }
    None
}

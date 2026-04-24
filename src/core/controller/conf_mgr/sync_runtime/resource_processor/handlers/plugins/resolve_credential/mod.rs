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
mod header_cert_auth;
mod hmac;
mod jwe;
mod jwt;
mod key_auth;
mod openid_connect;

pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) use basic_auth::resolve as resolve_basic_auth;
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) use header_cert_auth::resolve as resolve_header_cert_auth;
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) use hmac::resolve as resolve_hmac;
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) use jwe::resolve as resolve_jwe;
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) use jwt::resolve as resolve_jwt;
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) use key_auth::resolve as resolve_key_auth;
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) use openid_connect::resolve as resolve_openid_connect;

/// Read a UTF-8 value from a Secret, checking `data` (base64-decoded bytes)
/// first and falling back to `string_data`. Returns the first non-empty match
/// across the provided keys.
pub(super) fn read_secret_utf8(secret: &k8s_openapi::api::core::v1::Secret, keys: &[&str]) -> Option<String> {
    if let Some(data) = &secret.data {
        for key in keys {
            if let Some(bytes) = data.get(*key) {
                if let Ok(value) = String::from_utf8(bytes.0.clone()) {
                    if !value.trim().is_empty() {
                        return Some(value);
                    }
                }
            }
        }
    }
    if let Some(string_data) = &secret.string_data {
        for key in keys {
            if let Some(value) = string_data.get(*key) {
                if !value.trim().is_empty() {
                    return Some(value.clone());
                }
            }
        }
    }
    None
}

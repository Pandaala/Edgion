//! Per-plugin-type Secret resolution.
//!
//! Each submodule defines a single `resolve(ep, resource_ref, ctx)` function
//! that reads Kubernetes Secret data and populates the plugin's resolved
//! fields. `edgion_plugins.rs::parse` dispatches to them in sequence.
//!
//! The re-exports below expose each resolver under a disambiguated name
//! (`resolve_basic_auth`, `resolve_jwt`, ...) so call sites remain greppable
//! even though each submodule exports a locally simple `resolve`.

mod ai_guard;
mod basic_auth;
mod credential_injector;
mod csrf;
mod ext_proc;
mod forward_auth;
mod guardrail;
mod header_cert_auth;
mod hmac;
mod jwe;
mod jwt;
mod key_auth;
mod ldap_auth;
mod openid_connect;
mod wasm;

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use ai_guard::resolve as resolve_ai_guard;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use basic_auth::resolve as resolve_basic_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use credential_injector::{
    resolve as resolve_credential_injector, status_errors as credential_injector_status_errors,
};
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use csrf::resolve as resolve_csrf;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use ext_proc::resolve as resolve_ext_proc;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use forward_auth::resolve as resolve_forward_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use guardrail::resolve as resolve_guardrail;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use header_cert_auth::resolve as resolve_header_cert_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use hmac::resolve as resolve_hmac;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use jwe::{
    resolve as resolve_jwe, status_errors as jwe_status_errors,
};
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use jwt::resolve as resolve_jwt;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use key_auth::resolve as resolve_key_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use ldap_auth::resolve as resolve_ldap_auth;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use openid_connect::resolve as resolve_openid_connect;
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) use wasm::resolve as resolve_wasm;

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};

/// Resolve HMAC credentials for a `Condition::HmacAuth` leaf.
///
/// Condition-tree HMAC ignores credential metadata (same as the former RAP
/// walker). Calls into `hmac.rs`; do not duplicate that resolver.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve_hmac_config(
    config: &mut edgion_resources::resources::edgion_plugins::plugin_configs::HmacMatchConfig,
    ep_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    hmac::resolve_config(config, hmac::MetadataPolicy::Ignore, ep_namespace, resource_ref, ctx);
}

/// Read a UTF-8 value from a Secret's `data` map. Returns the first non-empty
/// match across the provided keys.
///
/// `SecretHandler::parse` normalizes `stringData` into `data` and clears
/// `stringData`, so Secrets coming from `secret_store` always have
/// `string_data == None`; only the `data` map needs to be inspected.
///
/// Visible to the whole `handlers` module so `edgion_plugins.rs`'s status
/// derivation (`resolve_wasm_refs`) can re-run the *exact* same usable-material
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

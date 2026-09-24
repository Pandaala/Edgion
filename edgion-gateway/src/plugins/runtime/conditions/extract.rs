//! Multi-valued identity extractors for the shared condition layer (D-26).
//!
//! `KeyGet` stays a single optional string. JWT claims and mTLS SAN lists can
//! yield several candidates; those reads live here so `JwtClaimMatch` /
//! `MtlsMatch` and `RequestRestriction` share one implementation.

use crate::plugins::http::common::auth_common::resolve_claim_path;
use crate::plugins::runtime::PluginSession;
use edgion_resources::resources::edgion_plugins::conditions::{MtlsField, SanType};

/// JWT claim candidates from ctx. Reads `jwt_claims` (jwt_auth) first, then
/// `oidc_claims` (openid_connect). Requires the authenticating plugin to run
/// earlier in the chain with `store_claims_in_ctx: true`; otherwise missing.
pub fn extract_jwt_claim_values(session: &dyn PluginSession, claim: &str) -> Vec<String> {
    let Some(raw) = session
        .get_ctx_var("jwt_claims")
        .or_else(|| session.get_ctx_var("oidc_claims"))
    else {
        return Vec::new();
    };
    let Ok(claims) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    match resolve_claim_path(&claims, claim) {
        Some(node) => json_node_to_candidates(node),
        None => Vec::new(),
    }
}

/// mTLS client-certificate candidates. Subject / CN yield 0 or 1 value; SAN
/// yields every matching entry (optionally filtered by `san_type`).
pub fn extract_mtls_values(session: &dyn PluginSession, field: MtlsField, san_type: Option<&SanType>) -> Vec<String> {
    match field {
        MtlsField::Subject => single(session.client_cert_info().map(|c| c.subject)),
        MtlsField::Cn => single(session.client_cert_info().and_then(|c| c.cn)),
        MtlsField::San => extract_san_values(session, san_type),
    }
}

/// Flatten a JSON node into match candidates. Scalars yield one value; arrays
/// yield each scalar element ("any element matches"); objects / null / nested
/// non-scalars yield nothing (missing).
fn json_node_to_candidates(node: &serde_json::Value) -> Vec<String> {
    match node {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Number(n) => vec![n.to_string()],
        serde_json::Value::Bool(b) => vec![b.to_string()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                serde_json::Value::Bool(b) => Some(b.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn single(v: Option<String>) -> Vec<String> {
    match v {
        Some(s) => vec![s],
        None => Vec::new(),
    }
}

/// SAN candidate values, optionally filtered/normalized by type.
///
/// SAN storage format (see `tls/runtime/backend/cert_extractor.rs`): DNS names
/// are bare; `IP:` / `email:` / `uri:` / `otherName:` carry a type prefix.
/// Untyped matching returns the raw stored strings; typed matching filters by
/// prefix and strips it. `DNS` = entries with no known prefix.
fn extract_san_values(session: &dyn PluginSession, san_type: Option<&SanType>) -> Vec<String> {
    let Some(cert) = session.client_cert_info() else {
        return Vec::new();
    };
    match san_type {
        None => cert.sans,
        Some(SanType::Dns) => cert
            .sans
            .into_iter()
            .filter(|s| !edgion_common::allowed_san::has_typed_san_display_prefix(s))
            .collect(),
        Some(SanType::Uri) => strip_san_prefix(cert.sans, "uri:"),
        Some(SanType::Email) => strip_san_prefix(cert.sans, "email:"),
    }
}

fn strip_san_prefix(sans: Vec<String>, prefix: &str) -> Vec<String> {
    sans.into_iter()
        .filter_map(|s| s.strip_prefix(prefix).map(|v| v.to_string()))
        .collect()
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/conditions/extract_test.rs"]
mod tests;

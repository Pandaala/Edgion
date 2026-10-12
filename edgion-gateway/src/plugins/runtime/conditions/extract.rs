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
/// lazily yields matching borrowed entries (optionally filtered by `san_type`).
/// Untyped SANs retain raw display prefixes; typed URI/email values strip them.
/// DNS classification uses the canonical typed-SAN display-prefix helper.
pub fn extract_mtls_values<'a>(
    session: &'a dyn PluginSession,
    field: MtlsField,
    san_type: Option<&SanType>,
) -> impl Iterator<Item = &'a str> {
    let san_type = san_type.copied();
    session.client_cert_info().into_iter().flat_map(move |cert| {
        let single = match field {
            MtlsField::Subject => Some(cert.subject.as_str()),
            MtlsField::Cn => cert.cn.as_deref(),
            MtlsField::San => None,
        };
        let sans = if matches!(field, MtlsField::San) {
            cert.sans.as_slice()
        } else {
            &[]
        };
        single
            .into_iter()
            .chain(sans.iter().filter_map(move |san| match san_type {
                None => Some(san.as_str()),
                Some(SanType::Dns) => {
                    (!edgion_common::allowed_san::has_typed_san_display_prefix(san)).then_some(san.as_str())
                }
                Some(SanType::Uri) => san.strip_prefix("uri:"),
                Some(SanType::Email) => san.strip_prefix("email:"),
            }))
    })
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

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/conditions/extract_test.rs"]
mod tests;

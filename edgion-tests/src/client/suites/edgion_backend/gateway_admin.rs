//! Gateway admin-API access shared by the EdgionBackend suites.
//!
//! Credential-reservation assertions need three things that are identical for
//! every suite: where the gateway admin API lives, how to authenticate to it,
//! and the immutable backend UID that the reservation bucket key digests. They
//! live here so a suite cannot quietly grow its own copy — the UID lookup in
//! particular has a fallback that is easy to drop and impossible to notice
//! until the assertion fails.

use crate::framework::TestContext;
use serde_json::Value;

/// Mirrors the gateway's private `ai::attempt_runtime::KEY_PREFIX`.
pub const CREDENTIAL_RESERVATION_KEY_PREFIX: &str = "edgion:aic:";

pub fn admin_base(ctx: &TestContext) -> String {
    std::env::var("EDGION_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host))
        .trim_end_matches('/')
        .to_string()
}

pub fn authed(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        Ok(token) if !token.trim().is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

/// The identity the gateway uses for an EdgionBackend in credential
/// reservation keys.
///
/// The fallback is load-bearing, not defensive: `metadata.uid` is empty in the
/// file-system config center the integration harness runs, and the gateway
/// itself falls back to the typed resource key there (`try_compile_backend` in
/// `edgion-gateway/src/ai/backend_store.rs`). A lookup that only accepted a
/// real UID would derive a bucket key that never exists.
pub async fn backend_uid(ctx: &TestContext, namespace: &str, name: &str) -> Result<String, String> {
    let response = authed(
        ctx.http_client
            .get(format!("{}/configclient/edgionbackend/list", admin_base(ctx))),
    )
    .send()
    .await
    .map_err(|error| format!("AI backend configclient request failed: {error}"))?;
    let status = response.status();
    let listed: Value = response
        .json()
        .await
        .map_err(|error| format!("invalid AI backend configclient response: {error}"))?;
    if !status.is_success() {
        return Err(format!("AI backend configclient returned {status}: {listed}"));
    }
    let backend = listed["data"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item.pointer("/metadata/namespace").and_then(Value::as_str) == Some(namespace)
                    && item.pointer("/metadata/name").and_then(Value::as_str) == Some(name)
            })
        })
        .ok_or_else(|| format!("AI backend {namespace}/{name} is not synced: {listed}"))?;
    Ok(backend
        .pointer("/metadata/uid")
        .and_then(Value::as_str)
        .filter(|uid| !uid.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("edgion.io/EdgionBackend/{namespace}/{name}")))
}

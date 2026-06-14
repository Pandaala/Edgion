//! Access log entry definition

use crate::plugins::StageLogs;
use crate::routes::grpc::GrpcMatchInfo;
use crate::{BackendContext, EdgionHttpContext, MatchInfo, RequestInfo};
use edgion_resources::EdgionStatus;
use serde::{ser::SerializeMap, Serialize, Serializer};
use std::collections::HashMap;

fn is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

/// `ctx_map` keys that carry categorically-sensitive content — full JWT/OIDC
/// claims JSON (`store_claims_in_ctx`) and decrypted JWE plaintext
/// (`store_payload_in_ctx`). These are stored in `ctx_map` so downstream
/// plugins can read them in-memory via `get_ctx_var`, but they must never be
/// projected into the access log.
///
/// Extension point: any new `ctx_map` key that stores claims, decrypted
/// payloads, tokens, or other secrets MUST be added here so the access-log
/// projection drops it.
const ACCESS_LOG_BLOCKED_CTX_KEYS: &[&str] = &["jwt_claims", "oidc_claims", "jwe_payload"];

#[inline]
fn is_blocked_ctx_key(key: &str) -> bool {
    ACCESS_LOG_BLOCKED_CTX_KEYS.contains(&key)
}

/// Serialize-time filter over the plugin `ctx_map`: flattens every entry into
/// the access-log JSON except the sensitive keys in
/// `ACCESS_LOG_BLOCKED_CTX_KEYS`. Allocation-free — no intermediate map is
/// rebuilt, which matters because the access log runs on the data-plane path.
pub struct FilteredCtx<'a>(&'a HashMap<String, String>);

impl FilteredCtx<'_> {
    /// Used by `skip_serializing_if`. Cheap underlying-emptiness check: when the
    /// map only holds blocked keys the flatten still emits nothing, so this is a
    /// fast-path skip rather than a correctness guard.
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for FilteredCtx<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (k, v) in self.0 {
            if !is_blocked_ctx_key(k) {
                map.serialize_entry(k, v)?;
            }
        }
        map.end()
    }
}

/// Unified match info that covers both HTTP and gRPC routes in access logs.
#[derive(Serialize)]
#[serde(untagged)]
pub enum RouteMatchInfo<'a> {
    Http(&'a MatchInfo),
    Grpc(&'a GrpcMatchInfo),
}

#[derive(Serialize)]
pub struct AccessLogEntry<'a> {
    #[serde(rename = "ts")]
    pub timestamp: i64,

    pub request_info: &'a RequestInfo,

    /// Entry-time raw URI (raw path + raw query). Populated only when
    /// `[access_log_options] include_raw_uri = true`. Carries the exact URI
    /// the client sent, before any path normalization.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub request_uri: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_info: Option<RouteMatchInfo<'a>>,

    #[serde(skip_serializing_if = "is_empty")]
    pub info: &'a [EdgionStatus],

    #[serde(skip_serializing_if = "is_empty")]
    pub errors: &'a [EdgionStatus],

    pub backend_context: Option<&'a BackendContext>,

    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub stage_logs: &'a [StageLogs],

    /// Plugin-set context variables (e.g. CtxSet writes here, request matchers read).
    /// Flattened into the response so callers can grep "key":"value" directly.
    /// Sensitive keys (`ACCESS_LOG_BLOCKED_CTX_KEYS`) are dropped at serialize time.
    #[serde(skip_serializing_if = "FilteredCtx::is_empty", flatten)]
    pub ctx: FilteredCtx<'a>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub conn_est: Option<bool>,
}

impl<'a> AccessLogEntry<'a> {
    #[inline]
    pub fn from_context(ctx: &'a EdgionHttpContext) -> Self {
        let match_info = if let Some(ru) = ctx.route_unit.as_ref() {
            Some(RouteMatchInfo::Http(&ru.matched_info))
        } else {
            ctx.grpc_route_unit
                .as_ref()
                .map(|gru| RouteMatchInfo::Grpc(&gru.matched_info))
        };

        let request_uri = if crate::cli::config::get_access_log_options().include_raw_uri
            && !ctx.request_info.entry_raw_path.is_empty()
        {
            Some(ctx.request_uri())
        } else {
            None
        };

        Self {
            timestamp: chrono::Utc::now().timestamp_millis(),
            request_info: &ctx.request_info,
            request_uri,
            match_info,
            info: &ctx.info,
            errors: &ctx.edgion_status,
            backend_context: ctx.backend_context.as_ref(),
            stage_logs: &ctx.stage_logs,
            ctx: FilteredCtx(&ctx.ctx_map),
            conn_est: None,
        }
    }

    /// Set connection established flag
    #[inline]
    pub fn set_conn_est(&mut self) {
        self.conn_est = Some(true);
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"ts":{},"errors":["AccessLogSerializeFailed"]}}"#,
                chrono::Utc::now().timestamp_millis()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_log_includes_acme_local_response_status() {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.status = Some(200);
        ctx.add_info(EdgionStatus::AcmeChallengeServed);

        let json = AccessLogEntry::from_context(&ctx).to_json();

        assert!(
            json.contains(r#""info":["AcmeChallengeServed"]"#),
            "access log must include ACME local response status, got: {json}"
        );
        assert!(
            !json.contains(r#""errors""#),
            "ACME local response should not be reported as an error, got: {json}"
        );
    }

    #[test]
    fn access_log_filters_sensitive_ctx_keys() {
        let mut ctx = EdgionHttpContext::new();
        // Sensitive keys written by jwt_auth / openid_connect / jwe_decrypt when
        // store_claims_in_ctx / store_payload_in_ctx is enabled.
        ctx.set_ctx_var("jwt_claims".to_string(), r#"{"sub":"alice"}"#.to_string());
        ctx.set_ctx_var(
            "oidc_claims".to_string(),
            r#"{"email":"alice@example.com"}"#.to_string(),
        );
        ctx.set_ctx_var("jwe_payload".to_string(), "decrypted-secret".to_string());
        // A non-sensitive plugin ctx key (e.g. CtxSet) must still be flattened.
        ctx.set_ctx_var("user_tier".to_string(), "gold".to_string());

        let json = AccessLogEntry::from_context(&ctx).to_json();

        for blocked in ["jwt_claims", "oidc_claims", "jwe_payload"] {
            assert!(
                !json.contains(blocked),
                "sensitive ctx key {blocked} must not appear in access log, got: {json}"
            );
        }
        assert!(
            !json.contains("alice") && !json.contains("decrypted-secret"),
            "sensitive ctx values must not leak into access log, got: {json}"
        );
        assert!(
            json.contains(r#""user_tier":"gold""#),
            "non-sensitive ctx keys must still flatten into access log, got: {json}"
        );
    }
}

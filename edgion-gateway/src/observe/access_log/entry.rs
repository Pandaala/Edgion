//! Access log entry definition

use crate::plugins::StageLogs;
use crate::routes::grpc::GrpcMatchInfo;
use crate::{BackendContext, EdgionHttpContext, MatchInfo, RequestInfo};
use edgion_resources::EdgionStatus;
use serde::{ser::SerializeMap, Serialize, Serializer};
use std::collections::{BTreeSet, HashMap};

fn is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

/// `ctx_map` keys that carry categorically-sensitive content — full JWT/OIDC
/// claims JSON (`store_claims_in_ctx`) and decrypted JWE plaintext
/// (`store_payload_in_ctx`). These are stored in `ctx_map` so downstream
/// plugins can read them in-memory via `get_ctx_var`, but they must never be
/// projected into the access log.
///
/// Context variables are now logged via a per-route **opt-in allowlist**
/// (`edgion.io/log-ctx-keys`); nothing reaches the log unless an operator
/// names it. This blocklist is a secondary, defense-in-depth guard: even if a
/// key here is explicitly allow-listed it is still dropped, so claims/payloads
/// cannot be logged by mistake. Any new `ctx_map` key that stores claims,
/// decrypted payloads, tokens, or other secrets SHOULD still be added here.
const ACCESS_LOG_BLOCKED_CTX_KEYS: &[&str] = &["jwt_claims", "oidc_claims", "jwe_payload"];

#[inline]
fn is_blocked_ctx_key(key: &str) -> bool {
    ACCESS_LOG_BLOCKED_CTX_KEYS.contains(&key)
}

/// Serialize-time projection of the plugin `ctx_map` into the access log.
///
/// Deny-by-default: a `ctx_map` entry is emitted only when its key is in the
/// route's opt-in `allow` allowlist (`edgion.io/log-ctx-keys`) **and** is not a
/// hard-blocked sensitive key. When `allow` is `None` (the default) nothing is
/// emitted. Allocation-free — no intermediate map is rebuilt, which matters
/// because the access log runs on the data-plane path.
pub struct AllowlistedCtx<'a> {
    map: &'a HashMap<String, String>,
    allow: Option<&'a BTreeSet<String>>,
}

impl AllowlistedCtx<'_> {
    /// Used by `skip_serializing_if`. Fast-path skip when the route opted no
    /// keys in or the `ctx_map` is empty; the precise "no allowed key present"
    /// case still serializes to an empty (omitted) flatten.
    fn is_empty(&self) -> bool {
        self.allow.is_none() || self.map.is_empty()
    }
}

impl Serialize for AllowlistedCtx<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(allow) = self.allow {
            for (k, v) in self.map {
                if allow.contains(k) && !is_blocked_ctx_key(k) {
                    map.serialize_entry(k, v)?;
                }
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
    /// Deny-by-default: only keys named in the route's `edgion.io/log-ctx-keys`
    /// allowlist are flattened in (and hard-blocked sensitive keys never are),
    /// so callers can grep allow-listed "key":"value" directly.
    #[serde(skip_serializing_if = "AllowlistedCtx::is_empty", flatten)]
    pub ctx: AllowlistedCtx<'a>,

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

        // Per-route opt-in allowlist of ctx_map keys to project into the log
        // (`edgion.io/log-ctx-keys`); absent on the matched route -> nothing logged.
        let log_ctx_keys = ctx
            .route_unit
            .as_ref()
            .and_then(|ru| ru.matched_info.log_ctx_keys.as_deref())
            .or_else(|| {
                ctx.grpc_route_unit
                    .as_ref()
                    .and_then(|gru| gru.matched_info.log_ctx_keys.as_deref())
            });

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
            ctx: AllowlistedCtx {
                map: &ctx.ctx_map,
                allow: log_ctx_keys,
            },
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
    fn access_log_omits_ctx_without_allowlist() {
        // Deny-by-default: with no matched route (hence no `log-ctx-keys`
        // allowlist), no ctx_map entry may reach the access log.
        let mut ctx = EdgionHttpContext::new();
        ctx.set_ctx_var("user_tier".to_string(), "gold".to_string());
        ctx.set_ctx_var("jwt_claims".to_string(), r#"{"sub":"alice"}"#.to_string());

        let json = AccessLogEntry::from_context(&ctx).to_json();

        assert!(
            !json.contains("user_tier") && !json.contains("gold"),
            "ctx vars must not be logged without an opt-in allowlist, got: {json}"
        );
        assert!(
            !json.contains("jwt_claims") && !json.contains("alice"),
            "sensitive ctx keys must never be logged, got: {json}"
        );
    }

    #[test]
    fn allowlisted_ctx_projects_only_allowed_non_blocked_keys() {
        let mut map = HashMap::new();
        map.insert("tenant_id".to_string(), "acme-corp".to_string());
        map.insert("user_tier".to_string(), "gold".to_string());
        // Sensitive keys stay hard-blocked even when explicitly allow-listed.
        map.insert("jwt_claims".to_string(), r#"{"sub":"alice"}"#.to_string());

        let allow: BTreeSet<String> = ["tenant_id", "jwt_claims"].iter().map(|s| s.to_string()).collect();
        let projected = AllowlistedCtx {
            map: &map,
            allow: Some(&allow),
        };
        let json = serde_json::to_string(&projected).unwrap();

        assert!(
            json.contains(r#""tenant_id":"acme-corp""#),
            "allow-listed non-sensitive ctx key must be projected, got: {json}"
        );
        assert!(
            !json.contains("user_tier"),
            "non-allow-listed ctx key must be dropped, got: {json}"
        );
        assert!(
            !json.contains("jwt_claims") && !json.contains("alice"),
            "blocked sensitive key must be dropped even when allow-listed, got: {json}"
        );
    }
}

//! Access log entry definition

use crate::observe::access_log::log_ext::LogExt;
use crate::plugins::StageLogs;
use crate::routes::grpc::GrpcMatchInfo;
use crate::{ai::GenAiSummary, BackendContext, EdgionHttpContext, MatchInfo, RequestInfo};
use edgion_resources::EdgionStatus;
use serde::Serialize;

fn is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub conn_est: Option<bool>,

    /// Composed upstream authority (Host + SNI) when the target Service carries an
    /// `upstreamAuthority` policy; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_authority: Option<&'a str>,

    /// Final AI usage and routing summary. Model-level detail belongs here,
    /// not in Prometheus labels. The type cannot carry request/response text or
    /// provider credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genai: Option<&'a GenAiSummary>,

    /// Operator-declared fields (`AccessLogExtern`), nested one level deep.
    ///
    /// Nested rather than flattened: the top-level namespace holds `errors` and
    /// `info`, both plausible field names, and a flattened field named `errors`
    /// would emit duplicate JSON keys and silently shadow the error array under
    /// a last-wins parser. (Note `path` / `status` / `host` are NOT top-level —
    /// they live under `request_info` — so they were never at risk.)
    #[serde(skip_serializing_if = "LogExt::is_empty")]
    pub ext: LogExt,
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
            conn_est: None,
            upstream_authority: ctx.upstream_authority.as_deref(),
            genai: ctx.genai_summary.as_ref(),
            ext: LogExt::default(),
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
    fn ctx_map_never_reaches_the_log_without_a_declaration() {
        // The annotation allowlist is gone: ctx vars reach the log only through
        // an AccessLogExtern `from: ctx` declaration, which populates `ext`.
        let mut ctx = EdgionHttpContext::new();
        ctx.set_ctx_var("user_tier".to_string(), "gold".to_string());
        ctx.set_ctx_var("jwt_claims".to_string(), r#"{"sub":"alice"}"#.to_string());

        let json = AccessLogEntry::from_context(&ctx).to_json();

        assert!(!json.contains("user_tier") && !json.contains("gold"), "got: {json}");
        assert!(!json.contains("jwt_claims") && !json.contains("alice"), "got: {json}");
        assert!(!json.contains(r#""ext""#), "empty ext must be omitted, got: {json}");
    }

    #[test]
    fn upstream_authority_is_logged_only_when_present() {
        let mut ctx = EdgionHttpContext::new();
        assert!(
            !AccessLogEntry::from_context(&ctx)
                .to_json()
                .contains("upstream_authority"),
            "absent authority must be omitted from the access log"
        );

        ctx.upstream_authority = Some("tenant-a.sandbox.internal".to_string());
        let json = AccessLogEntry::from_context(&ctx).to_json();
        assert!(json.contains("tenant-a.sandbox.internal"), "got: {json}");
    }
}

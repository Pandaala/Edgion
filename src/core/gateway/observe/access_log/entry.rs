//! Access log entry definition

use crate::core::gateway::plugins::StageLogs;
use crate::core::gateway::routes::grpc::GrpcMatchInfo;
use crate::core::gateway::{BackendContext, EdgionHttpContext, MatchInfo, RequestInfo};
use crate::types::EdgionStatus;
use serde::Serialize;
use std::collections::HashMap;

fn is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

fn map_is_empty(m: &&HashMap<String, String>) -> bool {
    m.is_empty()
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
        serialize_with = "crate::types::sensitive::masked_opt::serialize"
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
    #[serde(skip_serializing_if = "map_is_empty", flatten)]
    pub ctx: &'a HashMap<String, String>,

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

        let request_uri = if crate::core::gateway::cli::config::get_access_log_options().include_raw_uri
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
            ctx: &ctx.ctx_map,
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
}

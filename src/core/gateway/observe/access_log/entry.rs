//! Access log entry definition

use crate::core::gateway::cli::config::get_ctx_log_denylist;
use crate::core::gateway::plugins::StageLogs;
use crate::core::gateway::routes::grpc::GrpcMatchInfo;
use crate::types::{BackendContext, EdgionHttpContext, EdgionStatus, MatchInfo, RequestInfo};
use serde::Serialize;
use std::collections::HashMap;

fn is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

/// ASCII case-insensitive substring search — avoids the heap allocation that
/// `str::to_lowercase().contains()` would require on every call.
#[inline]
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_info: Option<RouteMatchInfo<'a>>,

    #[serde(skip_serializing_if = "is_empty")]
    pub errors: &'a [EdgionStatus],

    pub backend_context: Option<&'a BackendContext>,

    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub stage_logs: &'a [StageLogs],

    #[serde(skip_serializing_if = "Option::is_none")]
    pub conn_est: Option<bool>,

    /// Filtered subset of ctx_map: keys starting with `_` or matching denylist substrings
    /// (case-insensitive) are excluded to prevent PII/credential leakage in access logs.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub ctx: HashMap<String, String>,
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

        Self {
            timestamp: chrono::Utc::now().timestamp_millis(),
            request_info: &ctx.request_info,
            match_info,
            errors: &ctx.edgion_status,
            backend_context: ctx.backend_context.as_ref(),
            stage_logs: &ctx.stage_logs,
            conn_est: None,
            ctx: {
                let denylist = get_ctx_log_denylist();
                ctx.ctx_map
                    .iter()
                    .filter(|(k, _)| !k.starts_with('_') && !denylist.iter().any(|d| contains_ignore_ascii_case(k, d)))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            },
        }
    }

    /// Set connection established flag
    #[inline]
    pub fn set_conn_est(&mut self) {
        self.conn_est = Some(true);
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            tracing::error!("Failed to serialize access log: {}", e);
            "{}".to_string()
        })
    }

    pub fn to_combined(&self, latency_ms: u64) -> String {
        // Use the last upstream from backend_context for the combined log format
        let upstream_peer = self
            .backend_context
            .and_then(|bc| bc.upstreams.last())
            .map(|upstream| {
                let ip = upstream.ip.as_deref().unwrap_or("-");
                let port = upstream.port.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
                format!("{}:{}", ip, port)
            })
            .unwrap_or_else(|| "-".to_string());

        format!(
            r#"{} - - [{}] "{} {}" {} {} "{}""#,
            upstream_peer,
            chrono::DateTime::from_timestamp_millis(self.timestamp)
                .map(|dt| dt.format("%d/%b/%Y:%H:%M:%S %z").to_string())
                .unwrap_or_else(|| "-".to_string()),
            "GET",
            self.request_info.path,
            self.request_info
                .status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string()),
            latency_ms,
            self.request_info.x_trace_id.as_deref().unwrap_or("-"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::cli::config::{init_plugin_metadata_config, PluginMetadataConfig};
    use crate::types::EdgionHttpContext;

    fn make_ctx_with_keys(pairs: &[(&str, &str)]) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        for (k, v) in pairs {
            ctx.set_ctx_var(k.to_string(), v.to_string());
        }
        ctx
    }

    #[test]
    fn test_ctx_log_filters_denylist_keys() {
        init_plugin_metadata_config(PluginMetadataConfig::default());
        let ctx = make_ctx_with_keys(&[
            ("jwt_claims", r#"{"email":"user@example.com"}"#),
            ("oidc_claims", r#"{"sub":"u123"}"#),
            ("jwe_payload", "plaintext"),
            ("tenant_id", "abc"),
            ("user_id", "u456"),
        ]);
        let entry = AccessLogEntry::from_context(&ctx);
        assert!(!entry.ctx.contains_key("jwt_claims"), "jwt_claims must be filtered");
        assert!(!entry.ctx.contains_key("oidc_claims"), "oidc_claims must be filtered");
        assert!(!entry.ctx.contains_key("jwe_payload"), "jwe_payload must be filtered");
        assert!(entry.ctx.contains_key("tenant_id"), "tenant_id must be kept");
        assert!(entry.ctx.contains_key("user_id"), "user_id must be kept");
    }

    #[test]
    fn test_ctx_log_filters_underscore_prefix() {
        init_plugin_metadata_config(PluginMetadataConfig::default());
        let ctx = make_ctx_with_keys(&[("_private_key", "secret_value"), ("public_key_name", "visible")]);
        let entry = AccessLogEntry::from_context(&ctx);
        assert!(!entry.ctx.contains_key("_private_key"), "_private must be filtered");
        assert!(
            entry.ctx.contains_key("public_key_name"),
            "public_key_name must be kept"
        );
    }

    #[test]
    fn test_ctx_log_case_insensitive() {
        init_plugin_metadata_config(PluginMetadataConfig::default());
        let ctx = make_ctx_with_keys(&[("JWT_CLAIMS", "value"), ("Auth_Header", "bearer")]);
        let entry = AccessLogEntry::from_context(&ctx);
        assert!(
            !entry.ctx.contains_key("JWT_CLAIMS"),
            "JWT_CLAIMS must be filtered (case-insensitive)"
        );
        assert!(
            !entry.ctx.contains_key("Auth_Header"),
            "Auth_Header must be filtered (case-insensitive)"
        );
    }

    #[test]
    fn test_ctx_log_empty_ctx_stays_empty() {
        init_plugin_metadata_config(PluginMetadataConfig::default());
        let ctx = EdgionHttpContext::new();
        let entry = AccessLogEntry::from_context(&ctx);
        assert!(entry.ctx.is_empty());
    }
}

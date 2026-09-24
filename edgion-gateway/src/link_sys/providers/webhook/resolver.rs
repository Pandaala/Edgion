//! Webhook key resolver — resolve key values by calling external webhook services.
//!
//! This is the main entry point called by the WebhookKeyGet plugin.

use crate::link_sys::runtime::store::get_webhook_client;
use crate::observe::metrics::record_webhook_resolve_error;
use crate::plugins::http::common::outbound::call::{outbound_call, OutboundCallError, OutboundResponse};
use crate::plugins::http::common::outbound::request::OutboundRequestDiagnostics;
use crate::plugins::runtime::PluginSession;
use edgion_resources::common::WebhookExtract;

use super::manager::get_webhook_global_max;

/// Resolve a key value by calling an external webhook service.
///
/// This is the main entry point called by the WebhookKeyGet plugin.
///
/// The outbound request shape (method, path, query args, headers, cookies) is
/// defined entirely by the webhook resource's `request` block and applied
/// inside `outbound_call`. The consumer only supplies the `session` (for origin
/// and template lookups) and an optional `path_override`.
///
/// Missing-field diagnostics are returned through the caller-owned collector,
/// independently of whether a value is resolved; the plugin owns their emission.
pub async fn resolve_webhook_key(
    session: &dyn PluginSession,
    webhook_ref: &str,
    path_override: Option<&str>,
    extract: &WebhookExtract,
    diagnostics: &mut OutboundRequestDiagnostics,
) -> Option<String> {
    let Some(client) = get_webhook_client(webhook_ref).map(|client| client.managed_client()) else {
        record_webhook_resolve_error("not_found");
        return None;
    };

    match outbound_call(
        &client,
        session,
        path_override,
        get_webhook_global_max(),
        &[],
        diagnostics,
    )
    .await
    {
        Ok(resp) => {
            let is_success = client.classify(&resp);
            if is_success {
                extract_value_from_response(&resp, extract)
            } else {
                record_webhook_resolve_error("status_error");
                None
            }
        }
        Err(OutboundCallError::Unhealthy) => {
            record_webhook_resolve_error("unhealthy");
            None
        }
        Err(OutboundCallError::RateLimited) => {
            record_webhook_resolve_error("rate_limited");
            None
        }
        Err(OutboundCallError::AuthorityRejected) => {
            record_webhook_resolve_error("authority_rejected");
            None
        }
        Err(_) => {
            record_webhook_resolve_error("call_failed");
            None
        }
    }
}

// ============================================================
// Helpers
// ============================================================

/// Extract a value from an OutboundResponse based on WebhookExtract rules.
fn extract_value_from_response(result: &OutboundResponse, extract: &WebhookExtract) -> Option<String> {
    match extract {
        WebhookExtract::Header { name } => result
            .headers
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),

        WebhookExtract::Cookie { name } => {
            for cookie_header in result.headers.get_all("set-cookie") {
                if let Ok(val) = cookie_header.to_str() {
                    if let Some(pair) = val.split(';').next() {
                        let mut parts = pair.splitn(2, '=');
                        if let (Some(n), Some(v)) = (parts.next(), parts.next()) {
                            if n.trim() == name {
                                return Some(v.trim().to_string());
                            }
                        }
                    }
                }
            }
            None
        }

        WebhookExtract::Body { path } => {
            let body = std::str::from_utf8(&result.body).ok()?;
            let json: serde_json::Value = serde_json::from_str(body).ok()?;
            // Dot-path traversal: "data.user_id" → json["data"]["user_id"]
            let mut current = &json;
            for segment in path.split('.') {
                if let Ok(index) = segment.parse::<usize>() {
                    current = current.get(index)?;
                } else {
                    current = current.get(segment)?;
                }
            }
            match current {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            }
        }

        WebhookExtract::BodyText => {
            if result.body.is_empty() {
                None
            } else {
                std::str::from_utf8(&result.body).ok().map(|s| s.trim().to_string())
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/webhook/resolver_test.rs"]
mod tests;

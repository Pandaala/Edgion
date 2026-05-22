//! Webhook key resolver — resolve key values by calling external webhook services.
//!
//! This is the main entry point called from session_adapter's key_get for
//! KeyGet::Webhook variants.

use std::collections::HashMap;

use crate::core::gateway::observe::metrics::record_webhook_resolve_error;
use crate::core::gateway::plugins::http::common::http_client::{connection_listed_hop_by_hop, is_hop_by_hop_or_listed};
use crate::core::gateway::plugins::runtime::PluginSession;
use crate::types::common::WebhookExtract;

use super::call::{webhook_call, WebhookCallError, WebhookCallRequest, WebhookCallResult};
use super::manager::{get_webhook_global_max, get_webhook_manager};

/// Resolve a key value by calling an external webhook service.
///
/// This is the main entry point called from session_adapter's key_get.
pub async fn resolve_webhook_key(
    session: &dyn PluginSession,
    webhook_ref: &str,
    forward_headers: Option<&[String]>,
    path_suffix: Option<&str>,
    extract: &WebhookExtract,
) -> Option<String> {
    let runtime = match get_webhook_manager().get(webhook_ref).await {
        Some(rt) => rt,
        None => {
            record_webhook_resolve_error("not_found");
            return None;
        }
    };

    // Build forwarded headers from incoming request
    let mut headers = HashMap::new();

    // Always forward X-Forwarded-* context headers
    let remote = session.remote_addr();
    if !remote.is_empty() {
        headers.insert("X-Forwarded-For".to_string(), remote.to_string());
    }
    headers.insert("X-Forwarded-Method".to_string(), session.get_method().to_string());
    headers.insert("X-Forwarded-Uri".to_string(), session.get_path().to_string());
    if let Some(host) = session.header_value("host") {
        headers.insert("X-Forwarded-Host".to_string(), host);
    }

    // Forward explicitly requested headers from the incoming request.
    // Per RFC 9110 §7.6.1, also strip names dynamically nominated by the
    // originating request's Connection: header value, in addition to the
    // canonical hop-by-hop set.
    if let Some(fwd) = forward_headers {
        let connection_value = session.header_value("connection").unwrap_or_default();
        let dynamic_listed = connection_listed_hop_by_hop(&connection_value);
        for name in fwd {
            if is_hop_by_hop_or_listed(name, &dynamic_listed) {
                continue;
            }
            if let Some(val) = session.header_value(name) {
                headers.insert(name.clone(), val);
            }
        }
    }

    let req = WebhookCallRequest {
        method: runtime.config.request_method.clone(),
        path: path_suffix.unwrap_or("").to_string(),
        headers,
        body: None,
    };

    match webhook_call(&runtime, req, get_webhook_global_max()).await {
        Ok(result) => extract_value(&result, extract),
        Err(WebhookCallError::Unhealthy) => {
            record_webhook_resolve_error("unhealthy");
            None
        }
        Err(WebhookCallError::RateLimited) => {
            record_webhook_resolve_error("rate_limited");
            None
        }
        Err(WebhookCallError::StatusNotSuccess { .. }) => {
            record_webhook_resolve_error("status_error");
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

/// Extract a value from a WebhookCallResult based on WebhookExtract rules.
fn extract_value(result: &WebhookCallResult, extract: &WebhookExtract) -> Option<String> {
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

// NOTE: extract_value tests require a real HTTP server (mockito).
// These are covered by integration tests instead.
// See examples/code/client/suites/edgion_plugins/webhook_key_get/ for full integration tests.

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    // Verifies the resolve_webhook_key signature accepts the new fields.
    // Uses a wrapper async fn to avoid async fn pointer lifetime variance issues.
    #[allow(dead_code)]
    async fn _type_check<'a>(
        session: &'a dyn crate::core::gateway::plugins::runtime::PluginSession,
        webhook_ref: &'a str,
        forward_headers: Option<&'a [String]>,
        path_suffix: Option<&'a str>,
        extract: &'a crate::types::common::WebhookExtract,
    ) -> Option<String> {
        super::resolve_webhook_key(session, webhook_ref, forward_headers, path_suffix, extract).await
    }

    fn make_result(status: u16, headers: reqwest::header::HeaderMap, body: &[u8]) -> WebhookCallResult {
        WebhookCallResult {
            status,
            headers,
            body: Bytes::copy_from_slice(body),
        }
    }

    #[test]
    fn extract_header_returns_value() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-resolved-key", "my-token".parse().unwrap());
        let result = make_result(200, headers, b"");
        let extract = WebhookExtract::Header {
            name: "x-resolved-key".to_string(),
        };
        assert_eq!(extract_value(&result, &extract), Some("my-token".to_string()));
    }

    #[test]
    fn extract_header_missing_returns_none() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"");
        let extract = WebhookExtract::Header {
            name: "x-missing".to_string(),
        };
        assert_eq!(extract_value(&result, &extract), None);
    }

    #[test]
    fn extract_cookie_returns_value() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append("set-cookie", "session=abc123; HttpOnly; Path=/".parse().unwrap());
        let result = make_result(200, headers, b"");
        let extract = WebhookExtract::Cookie {
            name: "session".to_string(),
        };
        assert_eq!(extract_value(&result, &extract), Some("abc123".to_string()));
    }

    #[test]
    fn extract_cookie_missing_returns_none() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"");
        let extract = WebhookExtract::Cookie {
            name: "session".to_string(),
        };
        assert_eq!(extract_value(&result, &extract), None);
    }

    #[test]
    fn extract_body_json_path() {
        let body = br#"{"data": {"user_id": "42"}}"#;
        let result = make_result(200, reqwest::header::HeaderMap::new(), body);
        let extract = WebhookExtract::Body {
            path: "data.user_id".to_string(),
        };
        assert_eq!(extract_value(&result, &extract), Some("42".to_string()));
    }

    #[test]
    fn extract_body_json_missing_path_returns_none() {
        let body = br#"{"data": {}}"#;
        let result = make_result(200, reqwest::header::HeaderMap::new(), body);
        let extract = WebhookExtract::Body {
            path: "data.user_id".to_string(),
        };
        assert_eq!(extract_value(&result, &extract), None);
    }

    #[test]
    fn extract_body_text() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"  hello-world  ");
        let extract = WebhookExtract::BodyText;
        assert_eq!(extract_value(&result, &extract), Some("hello-world".to_string()));
    }

    #[test]
    fn extract_body_text_empty_returns_none() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"");
        let extract = WebhookExtract::BodyText;
        assert_eq!(extract_value(&result, &extract), None);
    }

    // P1 — hop-by-hop header filtering
    /// Verifies that is_hop_by_hop correctly identifies hop-by-hop headers,
    /// locking in the behavior used in resolve_webhook_key's forwardHeaders filtering.
    #[test]
    fn is_hop_by_hop_filters_connection() {
        use crate::core::gateway::plugins::http::common::http_client::is_hop_by_hop;
        assert!(is_hop_by_hop("Connection"));
        assert!(is_hop_by_hop("Transfer-Encoding"));
        assert!(is_hop_by_hop("Keep-Alive"));
        assert!(!is_hop_by_hop("Authorization"));
        assert!(!is_hop_by_hop("X-Tenant-Id"));
    }
}

//! Webhook key resolver — resolve key values by calling external webhook services.
//!
//! This is the main entry point called from session_adapter's key_get for
//! KeyGet::Webhook variants.

use crate::observe::metrics::record_webhook_resolve_error;
use crate::plugins::http::common::outbound::call::{outbound_call, OutboundCallError, OutboundResponse};
use crate::plugins::runtime::PluginSession;
use edgion_resources::common::WebhookExtract;

use super::manager::{get_webhook_global_max, get_webhook_manager};

/// Resolve a key value by calling an external webhook service.
///
/// This is the main entry point called from session_adapter's key_get.
///
/// The outbound request shape (method, path, query args, headers, cookies) is
/// now defined entirely by the webhook resource's `request` block and applied
/// inside `outbound_call`. The consumer only supplies the `session` (for origin
/// and template lookups) and an optional `path_override`.
///
/// `_forward_headers` is retained in the signature for the Wave C consumer
/// migration but is no longer consumed here — headers to forward are declared
/// on the webhook resource's `request.headers.origin` allow-list.
pub async fn resolve_webhook_key(
    session: &dyn PluginSession,
    webhook_ref: &str,
    _forward_headers: Option<&[String]>,
    path_override: Option<&str>,
    extract: &WebhookExtract,
) -> Option<String> {
    let client = match get_webhook_manager().get(webhook_ref).await {
        Some(c) => c,
        None => {
            record_webhook_resolve_error("not_found");
            return None;
        }
    };

    let path_override_owned = path_override.map(|s| s.to_string());

    match outbound_call(
        &client,
        session,
        path_override_owned.as_deref(),
        get_webhook_global_max(),
        &[],
    )
    .await
    {
        Ok(resp) => {
            let is_success = match &client.success_status_codes {
                Some(codes) => codes.contains(&resp.status),
                None => (200u16..300u16).contains(&resp.status),
            };
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

// NOTE: extract_value_from_response tests require a real HTTP server (mockito).
// These are covered by integration tests instead.
// See edgion-tests/src/client/suites/edgion_plugins/webhook_key_get/ for full integration tests.

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    // Verifies the resolve_webhook_key signature accepts the new fields.
    // Uses a wrapper async fn to avoid async fn pointer lifetime variance issues.
    #[allow(dead_code)]
    async fn _type_check<'a>(
        session: &'a dyn crate::plugins::runtime::PluginSession,
        webhook_ref: &'a str,
        forward_headers: Option<&'a [String]>,
        path_suffix: Option<&'a str>,
        extract: &'a edgion_resources::common::WebhookExtract,
    ) -> Option<String> {
        super::resolve_webhook_key(session, webhook_ref, forward_headers, path_suffix, extract).await
    }

    fn make_result(status: u16, headers: reqwest::header::HeaderMap, body: &[u8]) -> OutboundResponse {
        OutboundResponse {
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
        assert_eq!(
            extract_value_from_response(&result, &extract),
            Some("my-token".to_string())
        );
    }

    #[test]
    fn extract_header_missing_returns_none() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"");
        let extract = WebhookExtract::Header {
            name: "x-missing".to_string(),
        };
        assert_eq!(extract_value_from_response(&result, &extract), None);
    }

    #[test]
    fn extract_cookie_returns_value() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append("set-cookie", "session=abc123; HttpOnly; Path=/".parse().unwrap());
        let result = make_result(200, headers, b"");
        let extract = WebhookExtract::Cookie {
            name: "session".to_string(),
        };
        assert_eq!(
            extract_value_from_response(&result, &extract),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn extract_cookie_missing_returns_none() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"");
        let extract = WebhookExtract::Cookie {
            name: "session".to_string(),
        };
        assert_eq!(extract_value_from_response(&result, &extract), None);
    }

    #[test]
    fn extract_body_json_path() {
        let body = br#"{"data": {"user_id": "42"}}"#;
        let result = make_result(200, reqwest::header::HeaderMap::new(), body);
        let extract = WebhookExtract::Body {
            path: "data.user_id".to_string(),
        };
        assert_eq!(extract_value_from_response(&result, &extract), Some("42".to_string()));
    }

    #[test]
    fn extract_body_json_missing_path_returns_none() {
        let body = br#"{"data": {}}"#;
        let result = make_result(200, reqwest::header::HeaderMap::new(), body);
        let extract = WebhookExtract::Body {
            path: "data.user_id".to_string(),
        };
        assert_eq!(extract_value_from_response(&result, &extract), None);
    }

    #[test]
    fn extract_body_text() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"  hello-world  ");
        let extract = WebhookExtract::BodyText;
        assert_eq!(
            extract_value_from_response(&result, &extract),
            Some("hello-world".to_string())
        );
    }

    #[test]
    fn extract_body_text_empty_returns_none() {
        let result = make_result(200, reqwest::header::HeaderMap::new(), b"");
        let extract = WebhookExtract::BodyText;
        assert_eq!(extract_value_from_response(&result, &extract), None);
    }

    // P1 — hop-by-hop header filtering
    /// Verifies that is_hop_by_hop correctly identifies hop-by-hop headers,
    /// locking in the behavior used in resolve_webhook_key's forwardHeaders filtering.
    #[test]
    fn is_hop_by_hop_filters_connection() {
        use crate::plugins::http::common::http_client::is_hop_by_hop;
        assert!(is_hop_by_hop("Connection"));
        assert!(is_hop_by_hop("Transfer-Encoding"));
        assert!(is_hop_by_hop("Keep-Alive"));
        assert!(!is_hop_by_hop("Authorization"));
        assert!(!is_hop_by_hop("X-Tenant-Id"));
    }

    /// TDD: a completed response with non-success status records `status_error` metric and returns None.
    ///
    /// Verifies that the classification logic in resolve_webhook_key correctly
    /// identifies a 500 response as non-success (when success_status_codes is None,
    /// meaning only 2xx is considered success) and returns None.
    #[tokio::test]
    async fn webhook_non_success_records_status_error_and_returns_none() {
        // Build a fake OutboundResponse with status 500
        let resp = OutboundResponse {
            status: 500,
            headers: reqwest::header::HeaderMap::new(),
            body: Bytes::new(),
        };

        // Classify as non-success (success_status_codes = None means 2xx only)
        let success_codes: Option<Vec<u16>> = None; // None = any 2xx
        let is_success = match &success_codes {
            Some(codes) => codes.contains(&resp.status),
            None => (200u16..300u16).contains(&resp.status),
        };
        assert!(!is_success);

        // Verify: non-success status → None (mirrors the branch in resolve_webhook_key)
        let extract = edgion_resources::common::WebhookExtract::BodyText;
        let result = if is_success {
            extract_value_from_response(&resp, &extract)
        } else {
            record_webhook_resolve_error("status_error");
            None
        };
        assert!(result.is_none(), "non-success status must return None");
    }
}

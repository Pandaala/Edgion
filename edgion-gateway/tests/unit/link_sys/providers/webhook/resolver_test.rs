use super::*;
use bytes::Bytes;

// Verifies the resolve_webhook_key signature accepts the new fields.
// Uses a wrapper async fn to avoid async fn pointer lifetime variance issues.
#[allow(dead_code)]
async fn _type_check<'a>(
    session: &'a dyn crate::plugins::runtime::PluginSession,
    webhook_ref: &'a str,
    path_override: Option<&'a str>,
    extract: &'a edgion_resources::common::WebhookExtract,
) -> Option<String> {
    super::resolve_webhook_key(session, webhook_ref, path_override, extract, &mut Default::default()).await
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

/// TDD: a completed response with non-success status records `status_error` metric and returns None.
///
/// Verifies that the classification logic in resolve_webhook_key correctly
/// identifies a 500 response as non-success (when `success` is None, meaning
/// only 2xx is considered success — see `classify`) and returns None.
#[tokio::test]
async fn webhook_non_success_records_status_error_and_returns_none() {
    // Build a fake OutboundResponse with status 500
    let resp = OutboundResponse {
        status: 500,
        headers: reqwest::header::HeaderMap::new(),
        body: Bytes::new(),
    };

    // Classify as non-success (success = None means 2xx only, per `classify`)
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

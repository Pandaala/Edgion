//! Unified outbound HTTP call interface for webhook connections.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;

use super::health::{record_passive_result, should_halfopen_probe};
use super::runtime::WebhookRuntime;
use crate::types::resources::link_sys::webhook::WebhookAuth;

// ============================================================
// Public types
// ============================================================

/// Request parameters for a webhook call. Caller is responsible for all content.
pub struct WebhookCallRequest {
    /// HTTP method (e.g. "POST", "GET")
    pub method: String,
    /// Path suffix appended to the webhook's configured uri.
    /// Empty string = use uri as-is.
    pub path: String,
    /// Additional headers. Merged with config.default_headers; caller wins on conflict.
    pub headers: HashMap<String, String>,
    /// Request body bytes. None = no body.
    pub body: Option<Bytes>,
}

/// Successful webhook call result.
pub struct WebhookCallResult {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Bytes,
}

/// Webhook call error.
#[derive(Debug)]
pub enum WebhookCallError {
    /// Webhook is unhealthy and not in half-open state
    Unhealthy,
    /// Rate limited
    RateLimited,
    /// Request timed out
    Timeout,
    /// Connection failed
    Connect(String),
    /// Non-success HTTP status
    StatusNotSuccess { status: u16, body: Bytes },
    /// Response body exceeded size limit
    ResponseTooLarge,
    /// All retry attempts failed
    Exhausted(String),
}

// ============================================================
// Core call function
// ============================================================

/// Execute an outbound HTTP call using the given webhook runtime.
///
/// Handles: auth injection, default_headers merge, path construction,
/// retry, health tracking, rate limiting, response size capping.
pub async fn webhook_call(
    runtime: &WebhookRuntime,
    req: WebhookCallRequest,
    global_max_bytes: usize,
) -> Result<WebhookCallResult, WebhookCallError> {
    // 1. Health check gate
    if !runtime.healthy.load(Ordering::Relaxed) && !should_halfopen_probe(runtime) {
        return Err(WebhookCallError::Unhealthy);
    }

    // 2. Rate limit gate
    if let Some(ref counter) = runtime.rate_counter {
        if !counter.try_acquire() {
            return Err(WebhookCallError::RateLimited);
        }
    }

    // 3. Build URL
    let url = build_url(&runtime.config.uri, &req.path);

    // 4. Merge headers: default_headers < req.headers
    let merged = merge_default_headers(runtime.config.default_headers.clone().unwrap_or_default(), req.headers);

    // 5. Build reqwest HeaderMap
    let mut header_map = reqwest::header::HeaderMap::new();
    for (k, v) in &merged {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            header_map.insert(name, val);
        }
    }

    // 6. Inject auth
    inject_auth(&runtime.config.auth, &mut header_map);

    // 7. Build request and execute with retry
    let method: reqwest::Method = req.method.parse().unwrap_or(reqwest::Method::GET);
    let timeout = Duration::from_millis(runtime.config.timeout_ms);
    let max_attempts = 1 + runtime.config.retry.as_ref().map_or(0, |r| r.max_retries);
    let max_bytes = runtime.config.max_response_bytes.min(global_max_bytes);

    let mut last_error = String::new();

    for attempt in 0..max_attempts {
        if attempt > 0 {
            let base_ms = runtime.config.retry.as_ref().map_or(100, |r| r.retry_delay_ms);
            let delay = compute_retry_delay_ms(attempt, base_ms, 30_000);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        let mut builder = runtime
            .http_client
            .request(method.clone(), &url)
            .headers(header_map.clone())
            .timeout(timeout);

        if let Some(ref body) = req.body {
            builder = builder.body(body.clone());
        }

        match builder.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();

                // Check if we should retry on this status
                let should_retry = runtime
                    .config
                    .retry
                    .as_ref()
                    .is_some_and(|r| r.retry_on_status.contains(&status));
                if should_retry && attempt + 1 < max_attempts {
                    last_error = format!("status {status}");
                    continue;
                }

                // Passive health check
                let is_passive_fail = runtime
                    .config
                    .health_check
                    .as_ref()
                    .and_then(|hc| hc.passive.as_ref())
                    .map(|p| p.failure_status_codes.contains(&status))
                    .unwrap_or(false);
                record_passive_result(runtime, !is_passive_fail);

                // Check success
                let is_success = match &runtime.config.success_status_codes {
                    Some(codes) => codes.contains(&status),
                    None => (200..300).contains(&status),
                };
                if !is_success {
                    let body = read_body_limited(resp, max_bytes).await;
                    return Err(WebhookCallError::StatusNotSuccess { status, body });
                }

                // Read body
                let resp_headers = resp.headers().clone();
                let body = match resp.bytes().await {
                    Ok(b) => {
                        if b.len() > max_bytes {
                            return Err(WebhookCallError::ResponseTooLarge);
                        }
                        b
                    }
                    Err(e) => {
                        record_passive_result(runtime, false);
                        return Err(WebhookCallError::Connect(e.to_string()));
                    }
                };

                return Ok(WebhookCallResult {
                    status,
                    headers: resp_headers,
                    body,
                });
            }
            Err(e) => {
                let should_retry = runtime.config.retry.as_ref().is_some_and(|r| {
                    (r.retry_on_timeout && e.is_timeout()) || (r.retry_on_connect_error && e.is_connect())
                });
                last_error = e.to_string();
                if !should_retry || attempt + 1 >= max_attempts {
                    record_passive_result(runtime, false);
                    let err = if e.is_timeout() {
                        WebhookCallError::Timeout
                    } else {
                        WebhookCallError::Connect(e.to_string())
                    };
                    return Err(err);
                }
            }
        }
    }

    record_passive_result(runtime, false);
    Err(WebhookCallError::Exhausted(last_error))
}

// ============================================================
// Helpers (pub(crate) for tests)
// ============================================================

/// Concatenate base uri and path suffix.
pub(crate) fn build_url(base: &str, path: &str) -> String {
    if path.is_empty() {
        return base.to_string();
    }
    let base = base.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{base}{path}")
}

/// Merge default_headers (from config) with per-call headers.
/// Per-call headers win on conflict.
pub(crate) fn merge_default_headers(
    defaults: HashMap<String, String>,
    caller: HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = defaults;
    merged.extend(caller);
    merged
}

fn inject_auth(auth: &Option<WebhookAuth>, headers: &mut reqwest::header::HeaderMap) {
    let Some(auth) = auth else { return };
    match auth {
        WebhookAuth::Bearer { secret, .. } => {
            if let Some(token) = extract_secret_string(secret, "bearerToken") {
                if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                    headers.insert(reqwest::header::AUTHORIZATION, val);
                }
            }
        }
        WebhookAuth::Basic { secret, .. } => {
            let user = extract_secret_string(secret, "username").unwrap_or_default();
            let pass = extract_secret_string(secret, "password").unwrap_or_default();
            if !user.is_empty() {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
                if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}")) {
                    headers.insert(reqwest::header::AUTHORIZATION, val);
                }
            }
        }
        WebhookAuth::ApiKey {
            secret,
            r#in,
            header_name,
            query_param,
            ..
        } => {
            if let Some(key_val) = extract_secret_string(secret, "apiKey") {
                match r#in.as_str() {
                    "header" => {
                        if let Some(name) = header_name {
                            if let (Ok(hn), Ok(hv)) = (
                                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                                reqwest::header::HeaderValue::from_str(&key_val),
                            ) {
                                headers.insert(hn, hv);
                            }
                        }
                    }
                    "query" => {
                        // Query param injection requires URL mutation; deferred to a follow-up.
                        if let Some(param) = query_param {
                            tracing::warn!(
                                param = %param,
                                "ApiKey query auth is not yet implemented; the API key will NOT be sent"
                            );
                        }
                        let _ = key_val;
                    }
                    _ => {}
                }
            }
        }
        WebhookAuth::Hmac { .. } => {
            // HMAC signing is deferred: requires body bytes before building the request.
        }
    }
}

fn extract_secret_string(secret: &Option<k8s_openapi::api::core::v1::Secret>, key: &str) -> Option<String> {
    let secret = secret.as_ref()?;
    let data = secret.data.as_ref()?;
    let bytes = data.get(key)?;
    String::from_utf8(bytes.0.clone()).ok()
}

fn compute_retry_delay_ms(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    let exp = base_ms.saturating_mul(1u64 << attempt.min(10));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = nanos % (base_ms / 2).max(1);
    (exp + jitter).min(max_ms)
}

async fn read_body_limited(resp: reqwest::Response, max_bytes: usize) -> Bytes {
    match resp.bytes().await {
        Ok(b) => {
            if b.len() <= max_bytes {
                b
            } else {
                b.slice(..max_bytes)
            }
        }
        Err(_) => Bytes::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn webhook_call_request_default_path_is_empty() {
        let req = WebhookCallRequest {
            method: "GET".to_string(),
            path: String::new(),
            headers: Default::default(),
            body: None,
        };
        assert!(req.path.is_empty());
    }

    #[test]
    fn build_url_appends_path_suffix() {
        assert_eq!(
            build_url("https://api.example.com/v1", "/resolve"),
            "https://api.example.com/v1/resolve"
        );
        assert_eq!(
            build_url("https://api.example.com/v1", ""),
            "https://api.example.com/v1"
        );
        assert_eq!(build_url("https://api.example.com", "/"), "https://api.example.com/");
    }

    #[test]
    fn build_url_no_double_slash() {
        assert_eq!(
            build_url("https://api.example.com/", "/resolve"),
            "https://api.example.com/resolve"
        );
    }

    #[test]
    fn merge_headers_caller_wins() {
        let mut base = HashMap::new();
        base.insert("X-Source".to_string(), "base".to_string());
        base.insert("X-Only-Base".to_string(), "yes".to_string());

        let mut caller = HashMap::new();
        caller.insert("X-Source".to_string(), "caller".to_string());
        caller.insert("X-Caller".to_string(), "yes".to_string());

        let merged = merge_default_headers(base, caller);
        assert_eq!(merged.get("X-Source").map(String::as_str), Some("caller"));
        assert_eq!(merged.get("X-Only-Base").map(String::as_str), Some("yes"));
        assert_eq!(merged.get("X-Caller").map(String::as_str), Some("yes"));
    }
}

//! Unified outbound HTTP call interface for webhook connections.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;

use super::health::{record_passive_result, should_halfopen_probe};
use super::runtime::WebhookRuntime;
use edgion_resources::resources::link_sys::webhook::WebhookAuth;

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
#[derive(Debug)]
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
    let base = runtime.resolved_target.pick_url().ok_or(WebhookCallError::Unhealthy)?;
    let url = build_url(&base, &req.path);

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

                // Read body without buffering an oversized response: a declared
                // Content-Length over the cap is refused before reading any body,
                // and the streaming accumulator stops the moment the running total
                // would exceed the cap (peak memory <= cap + one chunk). Mirrors
                // `core/common/http_util::fetch_json_limited`.
                if let Some(cl) = resp.content_length() {
                    if cl > max_bytes as u64 {
                        return Err(WebhookCallError::ResponseTooLarge);
                    }
                }
                let resp_headers = resp.headers().clone();
                let (body, truncated) = match stream_body_capped(resp, max_bytes).await {
                    Ok(v) => v,
                    Err(e) => {
                        record_passive_result(runtime, false);
                        return Err(WebhookCallError::Connect(e.to_string()));
                    }
                };
                if truncated {
                    return Err(WebhookCallError::ResponseTooLarge);
                }

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
        WebhookAuth::Bearer { slot } => {
            if let Some(token) = extract_secret_string(&slot.secret, "bearerToken") {
                if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                    headers.insert(reqwest::header::AUTHORIZATION, val);
                }
            }
        }
        WebhookAuth::Basic { slot } => {
            let user = extract_secret_string(&slot.secret, "username").unwrap_or_default();
            let pass = extract_secret_string(&slot.secret, "password").unwrap_or_default();
            if !user.is_empty() {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
                if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}")) {
                    headers.insert(reqwest::header::AUTHORIZATION, val);
                }
            }
        }
        WebhookAuth::ApiKey {
            slot,
            r#in,
            header_name,
            query_param,
        } => {
            if let Some(key_val) = extract_secret_string(&slot.secret, "apiKey") {
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
                        let _ = (query_param, key_val);
                    }
                    _ => {}
                }
            }
        }
        WebhookAuth::Hmac { .. } => {
            // Unreachable: WebhookManager::upsert rejects auth.hmac at registration time
            // (signing is not yet implemented), so this arm never executes in practice.
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

/// Stream a response body, accumulating at most `max_bytes`. Returns the
/// collected bytes and whether the body exceeded the cap (further chunks were
/// discarded). Peak memory is at most `max_bytes` plus one chunk, regardless of
/// the actual body size.
async fn stream_body_capped(resp: reqwest::Response, max_bytes: usize) -> Result<(Bytes, bool), reqwest::Error> {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(8192.min(max_bytes));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = max_bytes - buf.len();
        if chunk.len() > remaining {
            buf.extend_from_slice(&chunk[..remaining]);
            return Ok((Bytes::from(buf), true));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((Bytes::from(buf), false))
}

/// Read a non-success response body for diagnostics, truncated to `max_bytes`.
/// A read error yields whatever was already buffered (empty on the first
/// failing chunk), matching the previous all-or-nothing behavior.
async fn read_body_limited(resp: reqwest::Response, max_bytes: usize) -> Bytes {
    stream_body_capped(resp, max_bytes)
        .await
        .map(|(b, _)| b)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
    use std::sync::Arc;

    use crate::link_sys::providers::webhook::runtime::WebhookResolvedTarget;
    use edgion_resources::resources::link_sys::webhook::{WebhookRetry, WebhookServiceConfig, WebhookTarget};

    // ============================================================
    // Test helpers
    // ============================================================

    fn make_runtime(uri: String, config_overrides: impl FnOnce(&mut WebhookServiceConfig)) -> WebhookRuntime {
        let resolved_target = WebhookResolvedTarget::Static(uri.clone());
        let mut config = WebhookServiceConfig {
            target: WebhookTarget::Uri { uri },
            ..Default::default()
        };
        config_overrides(&mut config);
        WebhookRuntime {
            config,
            http_client: Arc::new(reqwest::Client::new()),
            resolved_target,
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
            rate_counter: None,
        }
    }

    fn make_secret(key: &str, value: &str) -> k8s_openapi::api::core::v1::Secret {
        let mut data = std::collections::BTreeMap::new();
        data.insert(key.to_string(), k8s_openapi::ByteString(value.as_bytes().to_vec()));
        k8s_openapi::api::core::v1::Secret {
            data: Some(data),
            ..Default::default()
        }
    }

    fn simple_get_req() -> WebhookCallRequest {
        WebhookCallRequest {
            method: "GET".to_string(),
            path: String::new(),
            headers: HashMap::new(),
            body: None,
        }
    }

    /// Spawn a one-shot server that returns a 200 with the body sent via
    /// `Transfer-Encoding: chunked` (no `Content-Length`), so the size guard
    /// must be enforced by the streaming accumulator rather than the
    /// Content-Length pre-check.
    async fn spawn_chunked_server(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut throwaway = [0u8; 1024];
            let _ = sock.read(&mut throwaway).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                body.len()
            );
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(&body).await;
            let _ = sock.write_all(b"\r\n0\r\n\r\n").await;
            let _ = sock.shutdown().await;
        });
        format!("http://{addr}/")
    }

    // ============================================================
    // Existing tests
    // ============================================================

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

    // ============================================================
    // P0 — webhook_call() core flow tests
    // ============================================================

    /// 1. Unhealthy gate: runtime marked unhealthy with no half-open path → Unhealthy error.
    #[tokio::test]
    async fn unhealthy_gate_returns_unhealthy_error() {
        let runtime = make_runtime("http://127.0.0.1:1".to_string(), |_| {});
        // Mark unhealthy — no backoff config so should_halfopen_probe returns false.
        runtime.healthy.store(false, std::sync::atomic::Ordering::Relaxed);

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        assert!(matches!(result, Err(WebhookCallError::Unhealthy)), "Expected Unhealthy");
    }

    /// 2. RateLimited gate: exhausted sliding window counter → RateLimited error.
    #[tokio::test]
    async fn rate_limited_gate_returns_rate_limited_error() {
        use crate::link_sys::providers::webhook::runtime::SlidingWindowCounter;

        let mut runtime = make_runtime("http://127.0.0.1:1".to_string(), |_| {});
        // Create a counter with limit=0 so every acquire fails immediately.
        runtime.rate_counter = Some(SlidingWindowCounter::new(0, std::time::Duration::from_secs(1)));

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        assert!(
            matches!(result, Err(WebhookCallError::RateLimited)),
            "Expected RateLimited"
        );
    }

    /// 3. Successful 2xx call: wiremock returns 200 with body → Ok result with correct status + body.
    #[tokio::test]
    async fn successful_2xx_call_returns_ok_with_body() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello-webhook"))
            .mount(&server)
            .await;

        let runtime = make_runtime(format!("{}/resolve", server.uri()), |c| {
            // 1MB limit so body is not truncated
            c.max_response_bytes = 1024 * 1024;
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        let ok = result.expect("Expected Ok");
        assert_eq!(ok.status, 200);
        assert_eq!(ok.body.as_ref(), b"hello-webhook");
    }

    /// 4. StatusNotSuccess: wiremock returns 500 → Err(StatusNotSuccess { status: 500, .. }).
    #[tokio::test]
    async fn status_not_success_returns_error_with_status() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;

        let runtime = make_runtime(format!("{}/check", server.uri()), |c| {
            c.max_response_bytes = 1024 * 1024;
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        match result {
            Err(WebhookCallError::StatusNotSuccess { status, .. }) => {
                assert_eq!(status, 500);
            }
            Err(e) => panic!("Expected StatusNotSuccess(500), got Err({e:?})"),
            Ok(_) => panic!("Expected StatusNotSuccess(500), got Ok"),
        }
    }

    /// 5. ResponseTooLarge: wiremock returns body bigger than per-resource max_response_bytes → Err(ResponseTooLarge).
    #[tokio::test]
    async fn response_too_large_returns_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Body is 100 bytes; we'll set max_response_bytes to 10.
        let big_body = "x".repeat(100);
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_string(big_body))
            .mount(&server)
            .await;

        let runtime = make_runtime(format!("{}/big", server.uri()), |c| {
            c.max_response_bytes = 10;
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        assert!(
            matches!(result, Err(WebhookCallError::ResponseTooLarge)),
            "Expected ResponseTooLarge"
        );
    }

    /// 5b. Streaming guard: a chunked response (no Content-Length) larger than
    /// the cap is rejected by the streaming accumulator, not buffered in full.
    #[tokio::test]
    async fn chunked_response_exceeding_cap_returns_too_large() {
        let url = spawn_chunked_server(vec![b'z'; 4096]).await;
        let runtime = make_runtime(url, |c| {
            c.max_response_bytes = 128;
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        assert!(
            matches!(result, Err(WebhookCallError::ResponseTooLarge)),
            "Expected ResponseTooLarge from streaming guard, got {result:?}"
        );
    }

    /// 5c. A chunked response within the cap is read in full via the streaming
    /// path (no false truncation when Content-Length is absent).
    #[tokio::test]
    async fn chunked_response_within_cap_returns_full_body() {
        let body = b"streamed-body".to_vec();
        let url = spawn_chunked_server(body.clone()).await;
        let runtime = make_runtime(url, |c| {
            c.max_response_bytes = 1024;
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        let ok = result.expect("Expected Ok");
        assert_eq!(ok.status, 200);
        assert_eq!(ok.body.as_ref(), body.as_slice());
    }

    /// 6. Bearer auth injection: Authorization: Bearer tok123 is sent to the server.
    #[tokio::test]
    async fn bearer_auth_injects_authorization_header() {
        use edgion_resources::resources::common::SecretObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookAuth;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/auth"))
            .and(header("Authorization", "Bearer tok123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let secret = make_secret("bearerToken", "tok123");
        let runtime = make_runtime(format!("{}/auth", server.uri()), |c| {
            c.max_response_bytes = 1024;
            use edgion_resources::resources::common::SecretSlot;
            c.auth = Some(WebhookAuth::Bearer {
                slot: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "test-secret".to_string(),
                        namespace: None,
                    },
                    secret: Some(secret),
                },
            });
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        assert!(result.is_ok(), "Bearer auth call failed");
        assert_eq!(result.unwrap().status, 200);
    }

    /// 7. ApiKey header auth injection: X-API-Key: k123 is sent to the server.
    #[tokio::test]
    async fn api_key_header_auth_injects_custom_header() {
        use edgion_resources::resources::common::SecretObjectReference;
        use edgion_resources::resources::link_sys::webhook::WebhookAuth;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/key"))
            .and(header("X-API-Key", "k123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("authorized"))
            .mount(&server)
            .await;

        let secret = make_secret("apiKey", "k123");
        let runtime = make_runtime(format!("{}/key", server.uri()), |c| {
            c.max_response_bytes = 1024;
            use edgion_resources::resources::common::SecretSlot;
            c.auth = Some(WebhookAuth::ApiKey {
                slot: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "test-apikey".to_string(),
                        namespace: None,
                    },
                    secret: Some(secret),
                },
                r#in: "header".to_string(),
                header_name: Some("X-API-Key".to_string()),
                query_param: None,
            });
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        assert!(result.is_ok(), "ApiKey header auth call failed");
        assert_eq!(result.unwrap().status, 200);
    }

    /// 8. Retry on retry_on_status: first 500 then 200; final result is Ok, exactly 2 requests received.
    #[tokio::test]
    async fn retry_on_status_retries_and_succeeds() {
        use std::sync::atomic::AtomicU32 as StdAtomicU32;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // First request returns 500, second returns 200.
        // We use a counter to serve different responses.
        // wiremock supports priority: lower number = matched first.
        // Use `.up_to_n_times(1)` for the first 500, then unlimited 200.
        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(ResponseTemplate::new(200).set_body_string("success"))
            .with_priority(2)
            .mount(&server)
            .await;

        let _ = StdAtomicU32::new(0); // silence unused import warning

        let runtime = make_runtime(format!("{}/retry", server.uri()), |c| {
            c.max_response_bytes = 1024;
            c.retry = Some(WebhookRetry {
                max_retries: 1,
                retry_delay_ms: 0, // no delay in tests
                retry_on_timeout: false,
                retry_on_connect_error: false,
                retry_on_status: vec![500],
            });
        });

        let result = webhook_call(&runtime, simple_get_req(), 65536).await;
        assert!(result.is_ok(), "Expected Ok after retry");
        assert_eq!(result.unwrap().status, 200);

        // Verify exactly 2 requests were received (initial + 1 retry)
        let received = server.received_requests().await.unwrap();
        assert_eq!(
            received.len(),
            2,
            "Expected exactly 2 requests (1 initial + 1 retry), got {}",
            received.len()
        );
    }

    // ============================================================
    // P1 — defaultHeaders merge precedence in a real call
    // ============================================================

    /// 11. defaultHeaders merge precedence: caller wins on conflict, default kept when not overridden.
    #[tokio::test]
    async fn default_headers_merge_precedence_in_real_call() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Wiremock asserts: X-Source=caller (caller overrides default) AND X-Constant=yes (default kept).
        Mock::given(method("GET"))
            .and(path("/merge"))
            .and(header("X-Source", "caller"))
            .and(header("X-Constant", "yes"))
            .respond_with(ResponseTemplate::new(200).set_body_string("merged"))
            .mount(&server)
            .await;

        let runtime = make_runtime(format!("{}/merge", server.uri()), |c| {
            c.max_response_bytes = 1024;
            let mut defaults = HashMap::new();
            defaults.insert("X-Source".to_string(), "srv".to_string());
            defaults.insert("X-Constant".to_string(), "yes".to_string());
            c.default_headers = Some(defaults);
        });

        // Caller provides X-Source which should override the default.
        let mut caller_headers = HashMap::new();
        caller_headers.insert("X-Source".to_string(), "caller".to_string());

        let req = WebhookCallRequest {
            method: "GET".to_string(),
            path: String::new(),
            headers: caller_headers,
            body: None,
        };

        let result = webhook_call(&runtime, req, 65536).await;
        assert!(result.is_ok(), "defaultHeaders merge call failed");
        assert_eq!(result.unwrap().status, 200);
    }

    #[tokio::test]
    async fn pending_target_returns_unhealthy() {
        // WebhookResolvedTarget::Pending means endpoints haven't been injected yet.
        // The call must fail immediately with Unhealthy so that callers can surface
        // a meaningful error rather than panicking or hanging.
        let config = WebhookServiceConfig {
            target: WebhookTarget::Uri { uri: String::new() },
            ..Default::default()
        };
        let runtime = WebhookRuntime {
            config,
            http_client: Arc::new(reqwest::Client::new()),
            resolved_target: WebhookResolvedTarget::Pending,
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
            rate_counter: None,
        };
        let req = WebhookCallRequest {
            method: "GET".to_string(),
            path: String::new(),
            headers: Default::default(),
            body: None,
        };
        let result = webhook_call(&runtime, req, 65536).await;
        assert!(
            matches!(result, Err(WebhookCallError::Unhealthy)),
            "expected Unhealthy for Pending target, got {result:?}"
        );
    }
}

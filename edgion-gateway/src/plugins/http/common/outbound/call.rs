//! Core outbound HTTP call engine for the shared outbound-call infrastructure.
//!
//! [`outbound_call`] executes a single logical HTTP request using a
//! [`ManagedOutboundClient`]: it applies the health gate, rate gate, retry
//! loop, passive-health tracking, and response-size cap, then returns the raw
//! completed response for **any** HTTP status code — success classification is
//! left to the caller.

use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;

use super::client::{HealthState, ManagedOutboundClient};
use crate::plugins::runtime::PluginSession;

// ============================================================
// Public types
// ============================================================

/// A completed outbound HTTP response.
///
/// Returned by [`outbound_call`] for every completed exchange regardless of
/// status code. Success/failure classification is the caller's responsibility.
#[derive(Debug)]
pub struct OutboundResponse {
    /// HTTP status code (e.g. 200, 403, 500).
    pub status: u16,
    /// Response headers as received from the upstream.
    pub headers: reqwest::header::HeaderMap,
    /// Response body, capped to the effective `max_response_bytes` limit.
    pub body: Bytes,
}

/// Errors that prevent a completed HTTP response from being returned.
///
/// A non-2xx (or non-success) status code is **not** represented here — the
/// caller receives it as `Ok(OutboundResponse { status, .. })`.
#[derive(Debug)]
pub enum OutboundCallError {
    /// The client is currently marked unhealthy and not in half-open state.
    Unhealthy,
    /// The outbound rate limit was exhausted.
    RateLimited,
    /// The request timed out before a response was received.
    Timeout,
    /// A connection-level or request-build failure (includes SSRF rejection).
    Connect(String),
    /// The upstream response body exceeded the configured size cap.
    ResponseTooLarge,
    /// All retry attempts were exhausted after transport failures.
    Exhausted(String),
}

impl std::fmt::Display for OutboundCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unhealthy => write!(f, "outbound target is unhealthy"),
            Self::RateLimited => write!(f, "outbound rate limit exceeded"),
            Self::Timeout => write!(f, "outbound request timed out"),
            Self::Connect(msg) => write!(f, "outbound connection error: {msg}"),
            Self::ResponseTooLarge => write!(f, "outbound response body exceeded size cap"),
            Self::Exhausted(msg) => write!(f, "outbound retries exhausted: {msg}"),
        }
    }
}

// ============================================================
// Core call function
// ============================================================

/// Execute an outbound HTTP call using the given [`ManagedOutboundClient`].
///
/// ## Behavior
///
/// 1. **Health gate** — if `client.health.healthy` is `false` and no half-open
///    probe is permitted, returns `Err(Unhealthy)` immediately.
/// 2. **Rate gate** — if a rate counter is configured and the quota is
///    exhausted, returns `Err(RateLimited)` immediately.
/// 3. **Request build** — calls `client.compiled_request.build_request` with
///    the current session and optional path override. Build failures surface as
///    `Err(Connect)` (no passive-health record since no I/O occurred).
/// 4. **Retry loop** — executes up to `1 + retry.max_retries` attempts.
///    Between attempts an exponential-backoff delay (with jitter) is applied.
///    Retries are triggered by `retry.retry_on_status` (for HTTP responses) and
///    `retry.retry_on_timeout` / `retry.retry_on_connect_error` (for transport
///    errors).
/// 5. **Passive health** — on every completed HTTP response (success or not),
///    updates `client.health` based on `client.passive_health_check.failure_status_codes`.
/// 6. **Response size cap** — enforces `min(global_max_bytes, client.max_response_bytes)`.
///    A `Content-Length` pre-check stops before reading, and the streaming
///    accumulator stops mid-body; peak memory is at most cap + one chunk.
/// 7. **Return** — always returns `Ok(OutboundResponse { status, headers, body })`
///    for any completed HTTP exchange. The caller is responsible for checking
///    `status` against `client.success_status_codes`.
pub async fn outbound_call(
    client: &ManagedOutboundClient,
    session: &dyn PluginSession,
    path_override: Option<&str>,
    global_max_bytes: usize,
    extra_headers: &[(&str, &str)],
) -> Result<OutboundResponse, OutboundCallError> {
    // ── 1. Health gate ────────────────────────────────────────────────────────
    if !client.health.healthy.load(Ordering::Relaxed) && !should_halfopen_probe(client) {
        return Err(OutboundCallError::Unhealthy);
    }

    // ── 2. Rate gate ──────────────────────────────────────────────────────────
    if let Some(ref counter) = client.rate {
        if !counter.try_acquire() {
            return Err(OutboundCallError::RateLimited);
        }
    }

    // ── 3. Resolve base URL (with SSRF guard) and build the outbound request ─
    //    The request is built once and cloned per retry attempt.
    let base = client
        .dial_url()
        .map_err(|e| OutboundCallError::Connect(e.to_string()))?;

    let template_req =
        match client
            .compiled_request
            .build_request(session, path_override, &base, client.resolved_secrets.as_ref())
        {
            Ok(mut req) => {
                *req.timeout_mut() = Some(client.timeout);
                // Inject caller-supplied extra headers (e.g. X-Forwarded-* from ForwardAuth).
                // Silently skip headers with invalid names or values (data-plane rule).
                for &(name, val) in extra_headers {
                    if let (Ok(n), Ok(v)) = (
                        reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                        reqwest::header::HeaderValue::from_str(val),
                    ) {
                        req.headers_mut().insert(n, v);
                    }
                }
                req
            }
            // Build failure (missing required origin field, invalid URL/method) is a
            // request-construction error, not a backend failure, so we do not record
            // a passive health result.
            Err(e) => return Err(OutboundCallError::Connect(e)),
        };

    // ── 4. Execute with retry ─────────────────────────────────────────────────
    let max_attempts = 1 + client.retry.as_ref().map_or(0, |r| r.max_retries);
    let max_bytes = client.max_response_bytes.min(global_max_bytes);

    let mut last_error = String::new();

    for attempt in 0..max_attempts {
        if attempt > 0 {
            let base_ms = client.retry.as_ref().map_or(100, |r| r.retry_delay_ms);
            let delay = compute_retry_delay_ms(attempt, base_ms, 30_000);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        let req = match template_req.try_clone() {
            Some(r) => r,
            None => {
                return Err(OutboundCallError::Connect(
                    "outbound request is not cloneable".to_string(),
                ))
            }
        };

        let exec = tokio::time::timeout(client.timeout, client.http_client.execute(req)).await;

        match exec {
            Err(_elapsed) => {
                // tokio timeout fired
                let should_retry = client.retry.as_ref().is_some_and(|r| r.retry_on_timeout);
                last_error = "request timed out".to_string();
                if !should_retry || attempt + 1 >= max_attempts {
                    record_passive_result(&client.health, &client.passive_health_check, false);
                    return Err(OutboundCallError::Timeout);
                }
            }
            Ok(Err(e)) => {
                // Transport / connect error from reqwest
                let should_retry = client.retry.as_ref().is_some_and(|r| {
                    (r.retry_on_timeout && e.is_timeout()) || (r.retry_on_connect_error && e.is_connect())
                });
                last_error = e.to_string();
                if !should_retry || attempt + 1 >= max_attempts {
                    record_passive_result(&client.health, &client.passive_health_check, false);
                    let err = if e.is_timeout() {
                        OutboundCallError::Timeout
                    } else {
                        OutboundCallError::Connect(e.to_string())
                    };
                    return Err(err);
                }
            }
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();

                // Check if we should retry on this status
                let should_retry = client
                    .retry
                    .as_ref()
                    .is_some_and(|r| r.retry_on_status.contains(&status));
                if should_retry && attempt + 1 < max_attempts {
                    last_error = format!("status {status}");
                    continue;
                }

                // Passive health tracking (runs for all completed responses)
                let is_passive_fail = client
                    .passive_health_check
                    .as_ref()
                    .map(|p| p.failure_status_codes.contains(&status))
                    .unwrap_or(false);
                record_passive_result(&client.health, &client.passive_health_check, !is_passive_fail);

                // Content-Length pre-check to avoid buffering an oversized body
                if let Some(cl) = resp.content_length() {
                    if cl > max_bytes as u64 {
                        return Err(OutboundCallError::ResponseTooLarge);
                    }
                }

                let resp_headers = resp.headers().clone();
                let (body, truncated) = match stream_body_capped(resp, max_bytes).await {
                    Ok(v) => v,
                    Err(e) => {
                        record_passive_result(&client.health, &client.passive_health_check, false);
                        return Err(OutboundCallError::Connect(e.to_string()));
                    }
                };
                if truncated {
                    return Err(OutboundCallError::ResponseTooLarge);
                }

                return Ok(OutboundResponse {
                    status,
                    headers: resp_headers,
                    body,
                });
            }
        }
    }

    record_passive_result(&client.health, &client.passive_health_check, false);
    Err(OutboundCallError::Exhausted(last_error))
}

// ============================================================
// Helpers
// ============================================================

/// Compute a retry delay in milliseconds with exponential backoff and jitter.
///
/// Mirrors `webhook/call.rs::compute_retry_delay_ms` so both paths have the
/// same backoff profile.
fn compute_retry_delay_ms(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    let exp = base_ms.saturating_mul(1u64 << attempt.min(10));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = nanos % (base_ms / 2).max(1);
    (exp + jitter).min(max_ms)
}

/// Stream a response body, accumulating at most `max_bytes`.
///
/// Returns the collected bytes and whether the body exceeded the cap. Peak
/// memory is at most `max_bytes` plus one chunk regardless of the actual body
/// size.
pub(crate) async fn stream_body_capped(
    resp: reqwest::Response,
    max_bytes: usize,
) -> Result<(Bytes, bool), reqwest::Error> {
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

/// Record a passive health result for the given `HealthState`.
///
/// Mirrors the logic in `webhook/health.rs::record_passive_result` but operates
/// on [`HealthState`] + an `Option<PassiveHealthCheck>` so it can be used
/// without a full `WebhookRuntime`.
fn record_passive_result(
    health: &HealthState,
    passive: &Option<edgion_resources::resources::common::external_http::PassiveHealthCheck>,
    success: bool,
) {
    let Some(cfg) = passive.as_ref() else {
        return;
    };

    if success {
        health.passive_failures.store(0, Ordering::Relaxed);
        health.healthy.store(true, Ordering::Relaxed);
        health.backoff_sec.store(0, Ordering::Relaxed);
    } else {
        let failures = health.passive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= cfg.unhealthy_threshold && health.healthy.load(Ordering::Relaxed) {
            health.healthy.store(false, Ordering::Relaxed);
            if let Some(ref backoff) = cfg.backoff {
                health.backoff_sec.store(backoff.initial_sec, Ordering::Relaxed);
                health.last_halfopen.store(now_epoch_secs(), Ordering::Relaxed);
            }
        }
    }
}

/// Check if a half-open probe should be admitted for passive-only recovery.
///
/// Mirrors `webhook/health.rs::should_halfopen_probe` but operates on
/// [`HealthState`] + `Option<PassiveHealthCheck>`.
fn should_halfopen_probe(client: &ManagedOutboundClient) -> bool {
    if client.health.healthy.load(Ordering::Relaxed) {
        return false;
    }
    // If an active health task is running it handles recovery; no half-open needed.
    if client.health_task.is_some() {
        return false;
    }
    let passive = match client.passive_health_check.as_ref() {
        Some(p) => p,
        None => return false,
    };
    let backoff = match &passive.backoff {
        Some(b) => b,
        None => return false,
    };

    let now = now_epoch_secs();
    let last = client.health.last_halfopen.load(Ordering::Acquire);
    let interval = client.health.backoff_sec.load(Ordering::Relaxed);

    if interval == 0 || now >= last + interval {
        if client
            .health
            .last_halfopen
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            let next_backoff = ((interval as f64) * backoff.multiplier) as u64;
            client
                .health
                .backoff_sec
                .store(next_backoff.min(backoff.max_sec), Ordering::Relaxed);
            return true;
        }
    }
    false
}

/// Return the current wall-clock time as seconds since the Unix epoch.
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::plugins::http::common::http_client::get_http_client_arc;
    use crate::plugins::http::common::outbound::client::{HealthState, ManagedOutboundClient};
    use crate::plugins::http::common::outbound::request::CompiledRequest;
    use crate::plugins::http::common::outbound::target::ResolvedTarget;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Convert a wiremock server URI (e.g. `http://127.0.0.1:PORT`) to a
    /// `localhost`-based URL (`http://localhost:PORT`).
    ///
    /// `dial_url()` runs an SSRF guard on IP addresses. `127.0.0.1` is a loopback
    /// IP and is blocked. `localhost` is a DNS hostname — `extract_ip_literal`
    /// cannot parse it as a `SocketAddr`, so the literal-level SSRF check is skipped
    /// and `dial_url` returns `Ok`. `reqwest` then resolves `localhost` → `127.0.0.1`
    /// and connects to the mock server normally.
    fn localhost_uri(server_uri: &str) -> String {
        server_uri.replace("127.0.0.1", "localhost")
    }

    /// Build a minimal [`ManagedOutboundClient`] whose `ResolvedTarget` is a
    /// `Static` URL. The `url` must not be a loopback IP; use `localhost_uri`
    /// to convert wiremock URLs before passing them here.
    fn make_client(url: &str) -> ManagedOutboundClient {
        ManagedOutboundClient {
            http_client: get_http_client_arc(),
            resolved_target: ResolvedTarget::Static(url.to_string()),
            compiled_request: CompiledRequest::from_spec(&OutboundRequestSpec::default())
                .expect("default spec must compile"),
            health: HealthState::new(),
            rate: None,
            success_status_codes: None,
            max_response_bytes: 64 * 1024,
            timeout: Duration::from_secs(5),
            retry: None,
            egress_policy: None,
            passive_health_check: None,
            health_task: None,
            resolved_secrets: None,
        }
    }

    /// Build a mock session that returns the given HTTP method and request path.
    fn make_session(method: &str, path: &str) -> MockPluginSession {
        let mut s = MockPluginSession::new();
        s.expect_get_method().return_const(method.to_string());
        s.expect_get_request_path().return_const(path.to_string());
        s
    }

    // ── TDD tests ─────────────────────────────────────────────────────────────

    /// P0 TDD: a non-2xx response (403) must be returned as `Ok(OutboundResponse)`
    /// with headers intact, not as `Err(...)`.
    ///
    /// This is the key behavioral change vs. the old webhook_call: previously
    /// a 403 mapped to `Err(StatusNotSuccess { .. })` and dropped the headers.
    /// Now callers receive the full response and decide what to do.
    #[tokio::test]
    async fn non_success_returns_response_with_headers() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .respond_with(
                ResponseTemplate::new(403)
                    .append_header("www-authenticate", "Bearer")
                    .set_body_bytes(b"denied".to_vec()),
            )
            .mount(&server)
            .await;

        // wiremock binds to 127.0.0.1 which is blocked by dial_url's SSRF guard.
        // Use localhost_uri() to replace the IP with the hostname "localhost";
        // extract_ip_literal cannot parse a DNS name so the literal-level SSRF check
        // is skipped, and reqwest resolves "localhost" → 127.0.0.1 to reach the mock server.
        let client = make_client(&localhost_uri(&server.uri()));

        let session = make_session("GET", "/check");
        let resp = outbound_call(&client, &session, None, 4096, &[]).await.unwrap();
        assert_eq!(resp.status, 403);
        assert_eq!(
            resp.headers.get("www-authenticate").map(|v| v.to_str().unwrap_or("")),
            Some("Bearer")
        );
        assert_eq!(&resp.body[..], b"denied");
    }

    /// P0 TDD (timeout): a response that takes longer than `client.timeout` must
    /// produce `Err(OutboundCallError::Timeout)`.
    #[tokio::test]
    async fn timeout_returns_timeout_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(500))
                    .set_body_bytes(b"eventually".to_vec()),
            )
            .mount(&server)
            .await;

        let mut client = make_client(&localhost_uri(&server.uri()));
        client.timeout = Duration::from_millis(50);

        let session = make_session("GET", "/slow");
        let err = outbound_call(&client, &session, None, 4096, &[]).await.unwrap_err();
        assert!(
            matches!(err, OutboundCallError::Timeout),
            "expected Timeout, got {err:?}"
        );
    }

    /// Unhealthy gate: a client marked unhealthy (no backoff/halfopen) returns Unhealthy.
    #[tokio::test]
    async fn unhealthy_gate_returns_unhealthy_error() {
        let client = make_client("http://10.0.0.1:1");
        client.health.healthy.store(false, Ordering::Relaxed);

        let session = MockPluginSession::new();
        let result = outbound_call(&client, &session, None, 65536, &[]).await;
        assert!(
            matches!(result, Err(OutboundCallError::Unhealthy)),
            "expected Unhealthy, got {result:?}"
        );
    }

    /// Rate-limited gate: an exhausted counter returns RateLimited immediately.
    #[tokio::test]
    async fn rate_limited_gate_returns_rate_limited_error() {
        use crate::link_sys::providers::webhook::runtime::SlidingWindowCounter;

        let mut client = make_client("http://10.0.0.1:1");
        client.rate = Some(SlidingWindowCounter::new(0, Duration::from_secs(1)));

        let session = MockPluginSession::new();
        let result = outbound_call(&client, &session, None, 65536, &[]).await;
        assert!(
            matches!(result, Err(OutboundCallError::RateLimited)),
            "expected RateLimited, got {result:?}"
        );
    }

    /// ResponseTooLarge: response body exceeds `max_response_bytes` cap.
    #[tokio::test]
    async fn response_too_large_returns_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".repeat(200).to_vec()))
            .mount(&server)
            .await;

        let mut client = make_client(&localhost_uri(&server.uri()));
        client.max_response_bytes = 10; // well below 200 bytes

        let session = make_session("GET", "/big");
        let result = outbound_call(&client, &session, None, 65536, &[]).await;
        assert!(
            matches!(result, Err(OutboundCallError::ResponseTooLarge)),
            "expected ResponseTooLarge, got {result:?}"
        );
    }

    /// Successful 200 response is returned as Ok with correct body.
    #[tokio::test]
    async fn successful_200_returns_ok_with_body() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ok"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
            .mount(&server)
            .await;

        let client = make_client(&localhost_uri(&server.uri()));

        let session = make_session("GET", "/ok");
        let resp = outbound_call(&client, &session, None, 65536, &[]).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(&resp.body[..], b"hello");
    }

    /// Retry on status: first 503, then 200; final result is Ok after exactly 2 requests.
    #[tokio::test]
    async fn retry_on_status_retries_and_succeeds() {
        use edgion_resources::resources::common::external_http::WebhookRetry;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
            .with_priority(2)
            .mount(&server)
            .await;

        let mut client = make_client(&localhost_uri(&server.uri()));
        client.retry = Some(WebhookRetry {
            max_retries: 1,
            retry_delay_ms: 0,
            retry_on_timeout: false,
            retry_on_connect_error: false,
            retry_on_status: vec![503],
        });

        let session = make_session("GET", "/retry");
        let resp = outbound_call(&client, &session, None, 65536, &[]).await.unwrap();
        assert_eq!(resp.status, 200);

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 2, "expected 2 requests (initial + 1 retry)");
    }

    /// RC2 regression: `forward_all=true` must actually forward all non-hop-by-hop
    /// headers to the outbound target (including `Authorization`).
    ///
    /// Before the fix, `build_request` step 7 called `collect_list_fields` directly,
    /// which ignored `forward_all`. This test verifies the corrected path via
    /// `collect_headers`.
    #[tokio::test]
    async fn forward_all_actually_forwards_authorization_header() {
        use edgion_resources::resources::common::outbound_request::{ListFields, OutboundRequestSpec};
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/verify"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = ManagedOutboundClient {
            http_client: get_http_client_arc(),
            resolved_target: ResolvedTarget::Static(localhost_uri(&server.uri())),
            compiled_request: CompiledRequest::from_spec(&OutboundRequestSpec {
                headers: ListFields {
                    forward_all: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .expect("must compile"),
            health: HealthState::new(),
            rate: None,
            success_status_codes: None,
            max_response_bytes: 64 * 1024,
            timeout: Duration::from_secs(5),
            retry: None,
            egress_policy: None,
            passive_health_check: None,
            health_task: None,
            resolved_secrets: None,
        };

        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("GET".to_string());
        session.expect_get_request_path().return_const("/verify".to_string());
        session
            .expect_header_value()
            .withf(|name| name == "connection")
            .return_const(None::<String>);
        session
            .expect_request_headers()
            .return_const(vec![("authorization".to_string(), "Bearer test-token".to_string())]);

        let resp = outbound_call(&client, &session, None, 4096, &[]).await.unwrap();
        assert_eq!(resp.status, 200, "authorization header should have been forwarded");
    }

    /// RC3 regression: `${secretRef:...}` templates in custom headers must resolve
    /// at call time from `client.resolved_secrets`.
    ///
    /// Before the fix, `outbound_call` passed `None` for secrets to `build_request`,
    /// so every `${secretRef:...}` segment evaluated to an empty string.
    #[tokio::test]
    async fn secret_ref_in_custom_header_is_resolved_at_runtime() {
        use edgion_resources::resources::common::external_http::ResolvedSecrets;
        use edgion_resources::resources::common::outbound_request::{
            CustomField, CustomSource, ListFields, OutboundRequestSpec,
        };
        use std::collections::BTreeMap;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(header("authorization", "Bearer test-secret-value"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut secrets_map = BTreeMap::new();
        secrets_map.insert("ns/n:k".to_string(), "test-secret-value".to_string());

        let client = ManagedOutboundClient {
            http_client: get_http_client_arc(),
            resolved_target: ResolvedTarget::Static(localhost_uri(&server.uri())),
            compiled_request: CompiledRequest::from_spec(&OutboundRequestSpec {
                headers: ListFields {
                    custom: vec![CustomField {
                        name: "authorization".to_string(),
                        source: CustomSource::Template("Bearer ${secretRef:ns/n:k}".to_string()),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            })
            .expect("must compile"),
            health: HealthState::new(),
            rate: None,
            success_status_codes: None,
            max_response_bytes: 64 * 1024,
            timeout: Duration::from_secs(5),
            retry: None,
            egress_policy: None,
            passive_health_check: None,
            health_task: None,
            resolved_secrets: Some(ResolvedSecrets(secrets_map)),
        };

        let session = make_session("GET", "/api");
        let resp = outbound_call(&client, &session, None, 4096, &[]).await.unwrap();
        assert_eq!(resp.status, 200, "secretRef should have resolved at runtime");
    }
}

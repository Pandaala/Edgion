//! Core outbound HTTP call engine for the shared outbound-call infrastructure.
//!
//! [`outbound_call`] executes a single logical HTTP request using a
//! [`ManagedOutboundClient`]: it applies the health gate, rate gate, retry
//! loop, passive-health tracking, and response-size cap, then returns the raw
//! completed response for **any** HTTP status code — success classification is
//! left to the caller.

use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{Stream, StreamExt};

use edgion_resources::resources::common::success::SuccessOutcome;

use crate::backends::policy::backend_traffic::authority::render_authority;
use crate::backends::policy::backend_traffic::store::get_backend_traffic_policy_store;

use super::client::{HealthState, ManagedOutboundClient};
use super::request::{sync_outbound_body_framing, OutboundRequestDiagnostics};
use crate::plugins::runtime::PluginSession;

// ============================================================
// Public types
// ============================================================

/// A completed outbound HTTP response.
///
/// Returned by [`outbound_call`] for every completed exchange regardless of
/// status code. Success/failure classification is the caller's responsibility.
pub struct OutboundResponse {
    /// HTTP status code (e.g. 200, 403, 500).
    pub status: u16,
    /// Response headers as received from the upstream.
    pub headers: reqwest::header::HeaderMap,
    /// Response body, capped to the effective `max_response_bytes` limit.
    pub body: Bytes,
}

impl std::fmt::Debug for OutboundResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboundResponse")
            .field("status", &self.status)
            .field("headers", &"[redacted]")
            .field("body", &"[redacted]")
            .finish()
    }
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
    Connect,
    /// The upstream response body exceeded the configured size cap.
    ResponseTooLarge,
    /// All retry attempts were exhausted after transport failures.
    Exhausted,
    /// The request's composed upstream authority failed to render or validate
    /// (see `AuthorityError`). The call is aborted rather than falling back to
    /// the IP-based base URL, which would silently reach the wrong vhost.
    AuthorityRejected,
}

impl std::fmt::Display for OutboundCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unhealthy => write!(f, "outbound target is unhealthy"),
            Self::RateLimited => write!(f, "outbound rate limit exceeded"),
            Self::Timeout => write!(f, "outbound request timed out"),
            Self::Connect => write!(f, "outbound connection error"),
            Self::ResponseTooLarge => write!(f, "outbound response body exceeded size cap"),
            Self::Exhausted => write!(f, "outbound retries exhausted"),
            Self::AuthorityRejected => write!(f, "outbound request rejected: invalid upstream authority"),
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
///    for any completed HTTP exchange. The caller classifies it via
///    `client.classify(&resp)`, which reads `client.success`.
///
/// Supply fresh `diagnostics` for each logical call and emit it at the owning
/// logging boundary after awaiting, on both success and failure. It records
/// only missing warning fields actually evaluated during request construction.
pub async fn outbound_call(
    client: &ManagedOutboundClient,
    session: &dyn PluginSession,
    path_override: Option<&str>,
    global_max_bytes: usize,
    extra_headers: &[(&str, &str)],
    diagnostics: &mut OutboundRequestDiagnostics,
) -> Result<OutboundResponse, OutboundCallError> {
    outbound_call_with_body(
        client,
        session,
        path_override,
        global_max_bytes,
        extra_headers,
        None,
        diagnostics,
    )
    .await
}

/// Execute a call with an optional body override and caller-owned diagnostics.
/// See [`outbound_call`] for diagnostic lifetime and emission requirements.
pub async fn outbound_call_with_body(
    client: &ManagedOutboundClient,
    session: &dyn PluginSession,
    path_override: Option<&str>,
    global_max_bytes: usize,
    extra_headers: &[(&str, &str)],
    body_override: Option<bytes::Bytes>,
    diagnostics: &mut OutboundRequestDiagnostics,
) -> Result<OutboundResponse, OutboundCallError> {
    outbound_call_with_body_using(
        client,
        session,
        path_override,
        global_max_bytes,
        extra_headers,
        body_override,
        diagnostics,
        client.http_client.as_ref(),
    )
    .await
}

trait OutboundTransportError {
    fn is_timeout(&self) -> bool;
    fn is_connect(&self) -> bool;
}

impl OutboundTransportError for reqwest::Error {
    fn is_timeout(&self) -> bool {
        self.is_timeout()
    }

    fn is_connect(&self) -> bool {
        self.is_connect()
    }
}

trait OutboundHttpResponse {
    type BodyError;

    fn status(&self) -> u16;
    fn headers(&self) -> &reqwest::header::HeaderMap;
    fn content_length(&self) -> Option<u64>;
    fn bytes_stream(self) -> impl Stream<Item = Result<Bytes, Self::BodyError>>;
}

impl OutboundHttpResponse for reqwest::Response {
    type BodyError = reqwest::Error;

    fn status(&self) -> u16 {
        self.status().as_u16()
    }

    fn headers(&self) -> &reqwest::header::HeaderMap {
        self.headers()
    }

    fn content_length(&self) -> Option<u64> {
        self.content_length()
    }

    fn bytes_stream(self) -> impl Stream<Item = Result<Bytes, Self::BodyError>> {
        self.bytes_stream()
    }
}

trait OutboundTransport {
    type Response: OutboundHttpResponse;
    type Error: OutboundTransportError;

    async fn execute(&self, req: reqwest::Request) -> Result<Self::Response, Self::Error>;
}

impl OutboundTransport for reqwest::Client {
    type Response = reqwest::Response;
    type Error = reqwest::Error;

    async fn execute(&self, req: reqwest::Request) -> Result<Self::Response, Self::Error> {
        self.execute(req).await
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "private transport seam mirrors the public call parameters"
)]
async fn outbound_call_with_body_using<T: OutboundTransport>(
    client: &ManagedOutboundClient,
    session: &dyn PluginSession,
    path_override: Option<&str>,
    global_max_bytes: usize,
    extra_headers: &[(&str, &str)],
    body_override: Option<bytes::Bytes>,
    diagnostics: &mut OutboundRequestDiagnostics,
    transport: &T,
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
    //    The request is built once, moved into a sole attempt, or cloned for
    //    each attempt when retries are configured.
    let base = client.dial_url().map_err(|_| OutboundCallError::Connect)?;

    // Dynamic upstream authority: for a Service anchor carrying an
    // `upstreamAuthority` policy, swap the IP-based host for the composed
    // authority. Resolution of that authority is pinned to the same Service's
    // ready endpoints by `AnchorPinnedResolver` (installed in
    // `ManagedOutboundClient::from_parts`), so the dial destination is
    // unchanged — only Host/SNI move. This is never a fallback path: a
    // rejected authority aborts the call rather than silently dialing the
    // static/IP-based base URL.
    let base = match client.authority_service_key().and_then(|key| {
        let target = crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(key);
        get_backend_traffic_policy_store()
            .snapshot()
            .get_effective(&target)
            .and_then(|policy| policy.upstream_authority.clone())
    }) {
        Some(compiled) => match render_authority(&compiled, session) {
            Ok(authority) => replace_url_host(&base, &authority),
            Err(e) => {
                crate::observe::metrics::record_authority_rejected(e.reason());
                return Err(OutboundCallError::AuthorityRejected);
            }
        },
        None => base,
    };

    // Resolved once per call, not per retry attempt: the timeout template reads
    // the (frozen) incoming request via `session`, so its render cannot change
    // across attempts of the same logical call, and re-evaluating it every retry
    // would only risk a spurious extra fallback-metric increment if a transient
    // session read failed on a later attempt.
    let effective_timeout = resolve_timeout(client, session);

    let mut template_req = Some(
        match client.compiled_request.build_request(
            session,
            path_override,
            &base,
            client.resolved_secrets.as_ref(),
            diagnostics,
        ) {
            Ok(mut req) => {
                *req.timeout_mut() = Some(effective_timeout);
                // Inject caller-supplied extra headers (e.g. X-Forwarded-* from ForwardAuth).
                // Silently skip headers with invalid names or values (data-plane rule).
                // NOTE: these run after build_request, so a caller passing Content-Type
                // here would override the body-derived CT and bypass the admission-time
                // conflict check — no current caller does; keep it that way.
                for &(name, val) in extra_headers {
                    if let (Ok(n), Ok(v)) = (
                        reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                        reqwest::header::HeaderValue::from_str(val),
                    ) {
                        req.headers_mut().insert(n, v);
                    } else {
                        crate::observe::metrics::record_injected_header_skipped();
                    }
                }
                if let Some(body) = body_override {
                    *req.body_mut() = Some(body.into());
                }
                sync_outbound_body_framing(&mut req).map_err(|_| OutboundCallError::Connect)?;
                req
            }
            // Build failure (missing required origin field, invalid URL/method) is a
            // request-construction error, not a backend failure, so we do not record
            // a passive health result.
            Err(_) => return Err(OutboundCallError::Connect),
        },
    );

    // ── 4. Execute with retry ─────────────────────────────────────────────────
    let max_attempts = 1 + client.retry.as_ref().map_or(0, |r| r.max_retries);
    let max_bytes = client.max_response_bytes.min(global_max_bytes);

    // Set by the `Retry-After` handling below when a response carries a
    // within-cap `Retry-After` value; consumed (and cleared) by the
    // top-of-loop sleep on the next iteration so the header's delay is used
    // instead of the normal backoff — exactly once, never in addition to it.
    let mut next_delay_override: Option<u64> = None;

    for attempt in 0..max_attempts {
        if attempt > 0 {
            if let Some(retry) = client.retry.as_ref() {
                let delay = next_delay_override
                    .take()
                    .unwrap_or_else(|| compute_retry_delay_ms(attempt, retry));
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }

        let Some(req) = request_for_attempt(&mut template_req, max_attempts) else {
            return Err(OutboundCallError::Connect);
        };

        let exec = tokio::time::timeout(effective_timeout, transport.execute(req)).await;

        match exec {
            Err(_elapsed) => {
                // tokio timeout fired
                // `retry_on_timeout` is resolved to a per-consumer default (webhook:
                // false, forward_auth: true) in `ManagedOutboundClient::from_parts`;
                // the `unwrap_or(false)` here is a defensive fallback for a `client`
                // built outside `from_parts` (e.g. constructed directly in tests).
                let retry_on_timeout = client.retry.as_ref().and_then(|r| r.retry_on_timeout).unwrap_or(false);
                let should_retry = client.retry.is_some() && retry_on_timeout;
                if !should_retry || attempt + 1 >= max_attempts {
                    record_passive_result(&client.health, &client.passive_health_check, false);
                    return Err(OutboundCallError::Timeout);
                }
            }
            Ok(Err(e)) => {
                // Transport / connect error from reqwest
                // `retry_on_timeout` is resolved to a per-consumer default (webhook:
                // false, forward_auth: true) in `ManagedOutboundClient::from_parts`;
                // the `unwrap_or(false)` here is a defensive fallback for a `client`
                // built outside `from_parts` (e.g. constructed directly in tests).
                let retry_on_timeout = client.retry.as_ref().and_then(|r| r.retry_on_timeout).unwrap_or(false);
                let should_retry = client.retry.as_ref().is_some_and(|r| {
                    (retry_on_timeout && e.is_timeout()) || (r.retry_on_connect_error && e.is_connect())
                });
                if !should_retry || attempt + 1 >= max_attempts {
                    record_passive_result(&client.health, &client.passive_health_check, false);
                    let err = if e.is_timeout() {
                        OutboundCallError::Timeout
                    } else {
                        OutboundCallError::Connect
                    };
                    return Err(err);
                }
            }
            Ok(Ok(resp)) => {
                let status = resp.status();

                // Status-based retry eligibility.
                let mut status_retry = client
                    .retry
                    .as_ref()
                    .is_some_and(|r| r.retry_on_status.contains(&status));

                // `Retry-After` (headers only; opt-in; only for 429/503 already eligible).
                // An over-cap value means "stop and return as-is" and must win over any
                // body-predicate retry, so it is tracked separately.
                let mut retry_after_stop = false;
                if status_retry && attempt + 1 < max_attempts {
                    if let Some(retry) = client.retry.as_ref() {
                        if retry.honor_retry_after && (status == 429 || status == 503) {
                            match parse_retry_after_ms(resp.headers(), std::time::SystemTime::now()) {
                                Some(ra_ms) if ra_ms > retry.max_delay_ms => {
                                    // Backend asked for longer than we're willing to
                                    // block: stop retrying and return this response
                                    // as-is via the shared terminal-response path below.
                                    status_retry = false;
                                    retry_after_stop = true;
                                }
                                Some(ra_ms) => {
                                    next_delay_override = Some(ra_ms);
                                }
                                None => {
                                    // No/unparseable header: fall back to normal backoff.
                                }
                            }
                        }
                    }
                }

                // Retrying on status: the body is irrelevant to the decision. Do NOT
                // stream it — a large error page must not turn a retryable status into
                // ResponseTooLarge (`max_response_bytes` commonly defaults far below a
                // typical error page, e.g. 1024 bytes).
                if status_retry && attempt + 1 < max_attempts {
                    // A retried attempt is NOT terminal: do not record passive health here.
                    continue;
                }

                // Not status-retrying (either the status is fine, or this is the final
                // attempt so the response must be returned as-is): stream the (capped)
                // body — it is needed for body-predicate classification and/or to
                // return to the caller.
                if let Some(cl) = resp.content_length() {
                    if cl > max_bytes as u64 {
                        // A response that cannot be read within the size cap is a
                        // genuine call failure from the caller's perspective, even
                        // though the status itself might otherwise have counted as a
                        // passive-health success — recording a FAILURE here is
                        // deliberate, not an oversight.
                        record_passive_result(&client.health, &client.passive_health_check, false);
                        return Err(OutboundCallError::ResponseTooLarge);
                    }
                }
                let resp_headers = resp.headers().clone();
                let Ok((body, truncated)) = stream_body_chunks_capped(resp.bytes_stream(), max_bytes).await else {
                    record_passive_result(&client.health, &client.passive_health_check, false);
                    return Err(OutboundCallError::Connect);
                };
                if truncated {
                    // Same deliberate FAILURE-recording rationale as the content-length
                    // pre-check above: the call could not be completed within the cap.
                    record_passive_result(&client.health, &client.passive_health_check, false);
                    return Err(OutboundCallError::ResponseTooLarge);
                }
                let completed = OutboundResponse {
                    status,
                    headers: resp_headers,
                    body,
                };

                // Body-predicate classification drives body-retry — gated on the BODY
                // dimension only. `client.classify()` folds the status check into its
                // bool result, so using it here would let `retry_on_body_failure`
                // silently widen to retry ANY non-success status (e.g. a 404 outside
                // `retry_on_status`); `classify_detail` keeps the two dimensions separate.
                let outcome = client.classify_detail(&completed);
                let body_retry = !retry_after_stop
                    && client.retry.as_ref().is_some_and(|r| r.retry_on_body_failure)
                    && outcome == SuccessOutcome::BodyFail;

                if body_retry && attempt + 1 < max_attempts {
                    // A retried attempt is NOT terminal: do not record passive health here.
                    continue;
                }

                // Terminal response classified as a body-predicate failure: this is the
                // "200 OK but error in the body" case, otherwise invisible on the data
                // plane (no tracing is allowed here — the metric is the only signal).
                if outcome == SuccessOutcome::BodyFail {
                    crate::observe::metrics::record_outbound_success_predicate_fail();
                }

                // Terminal response: passive health keys off status only.
                let is_passive_fail = client
                    .passive_health_check
                    .as_ref()
                    .map(|p| p.failure_status_codes.contains(&status))
                    .unwrap_or(false);
                record_passive_result(&client.health, &client.passive_health_check, !is_passive_fail);

                return Ok(completed);
            }
        }
    }

    // Unreachable in practice: `max_attempts >= 1` always (it's `1 + max_retries`),
    // and on the final iteration every branch above returns unconditionally —
    // `status_retry`/`body_retry` are gated on `attempt + 1 < max_attempts`, and
    // the timeout/connect-error arms return whenever `attempt + 1 >= max_attempts`.
    // Kept as a defensive fallback (not `unreachable!()`) rather than deleted: a
    // `for` loop's value type is `()`, so the compiler cannot statically prove the
    // loop always returns internally, and this is data-plane code where a future
    // change that broke the invariant should degrade to a safe error response
    // instead of panicking a worker.
    record_passive_result(&client.health, &client.passive_health_check, false);
    Err(OutboundCallError::Exhausted)
}

// ============================================================
// Helpers
// ============================================================

/// Resolve the effective per-call timeout: the template wins when it renders to a
/// parseable value at or above `MIN_OUTBOUND_TIMEOUT_MS` (clamped to the upper
/// ceiling); otherwise the static `timeout` is used.
///
/// The lower bound guards against a tiny-but-nonzero render (e.g. "1") pinning
/// every attempt to an unusably short deadline — a reliable timeout-driven
/// failure on every call. Falling back to the static `timeout` (rather than
/// raising the render to the floor) matches the
/// existing fallback semantics for a non-numeric/zero/empty render, so callers
/// only need to reason about one fallback path.
///
/// Called once per `outbound_call` (before the retry loop) rather than once per
/// retry attempt: a retry re-sends the same logical request built from the same
/// frozen incoming session, so the template would render identically on every
/// attempt anyway, and computing it once avoids re-incrementing the fallback
/// metric on every retried attempt of a call whose template already fell back.
fn resolve_timeout(client: &ManagedOutboundClient, session: &dyn PluginSession) -> Duration {
    use edgion_resources::resources::common::external_http::{MAX_OUTBOUND_TIMEOUT_MS, MIN_OUTBOUND_TIMEOUT_MS};
    if let Some(tpl) = client.timeout_template.as_ref() {
        use crate::link_sys::template::{render_template, RenderPolicy, SecretCapability};
        if let Ok(rendered) = render_template(tpl, session, RenderPolicy::EMPTY, SecretCapability::Denied) {
            let rendered = rendered.into_string();
            if let Ok(ms) = rendered.trim().parse::<u64>() {
                if ms >= MIN_OUTBOUND_TIMEOUT_MS {
                    return Duration::from_millis(ms.min(MAX_OUTBOUND_TIMEOUT_MS));
                }
            }
        }
        crate::observe::metrics::record_outbound_template_fallback("timeout_ms");
    }
    client.timeout
}

/// Select the request for one outbound attempt.
///
/// A logical call with one attempt transfers ownership directly. Retry-capable
/// calls retain the template and clone it for every attempt, preserving the
/// existing rejection of non-replayable request bodies.
fn request_for_attempt(template_req: &mut Option<reqwest::Request>, max_attempts: u32) -> Option<reqwest::Request> {
    if max_attempts == 1 {
        template_req.take()
    } else {
        template_req.as_ref()?.try_clone()
    }
}

/// Compute a retry delay in milliseconds for `attempt` (1 = first retry).
///
/// `fixed`  → `retry_delay_ms`.
/// `exp`    → `retry_delay_ms << (attempt-1)`, saturating.
/// The result is capped to `max_delay_ms` BEFORE jitter, so jitter (a
/// `[d/2, d]` band) can never push the delay above the cap.
fn compute_retry_delay_ms(
    attempt: u32,
    retry: &edgion_resources::resources::common::external_http::WebhookRetry,
) -> u64 {
    use edgion_resources::resources::common::external_http::BackoffPolicy;
    use rand::Rng;

    let base = retry.retry_delay_ms;
    let raw = match retry.backoff_policy {
        BackoffPolicy::Fixed => base,
        BackoffPolicy::Exponential => {
            let shift = attempt.saturating_sub(1).min(20);
            base.saturating_mul(1u64 << shift)
        }
    };
    let capped = raw.min(retry.max_delay_ms);
    if retry.jitter {
        rand::rng().random_range((capped / 2)..=capped)
    } else {
        capped
    }
}

/// Replace `base`'s host with `authority`, keeping the scheme and port.
///
/// `base` is always a well-formed URL produced by `ManagedOutboundClient::dial_url`
/// (built from `ResolvedTarget::pick_url`), and `authority` is `<validated label>
/// <fixed suffix>` — a DNS label already checked by `validate_label` in
/// `render_authority`, concatenated with a compile-time-constant suffix — so
/// neither `Url::parse` nor `set_host` can fail in practice. `unwrap_or_else`
/// falls back to the unmodified `base` rather than panicking, since this runs on
/// the data-plane request path.
pub(crate) fn replace_url_host(base: &str, authority: &str) -> String {
    reqwest::Url::parse(base)
        .and_then(|mut url| url.set_host(Some(authority)).map(|()| url))
        .map(|url| url.to_string())
        .unwrap_or_else(|_| base.to_string())
}

/// Parse a `Retry-After` header into milliseconds. Accepts delta-seconds
/// (`"2"`) and HTTP-date; a past date yields 0. Returns `None` if the header
/// is absent or unparseable.
fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap, now: std::time::SystemTime) -> Option<u64> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    let duration = edgion_common::time::parse_retry_after(raw, now)?;
    Some(duration.as_millis().min(u64::MAX as u128) as u64)
}

/// Build a [`WebhookRetry`](edgion_resources::resources::common::external_http::WebhookRetry)
/// with the given `retry_delay_ms` base and otherwise-sane test defaults
/// (exponential backoff, 10s cap, no jitter).
#[cfg(test)]
pub(crate) fn tests_retry(base: u64) -> edgion_resources::resources::common::external_http::WebhookRetry {
    use edgion_resources::resources::common::external_http::{BackoffPolicy, WebhookRetry};
    WebhookRetry {
        max_retries: 3,
        retry_delay_ms: base,
        retry_on_timeout: None,
        retry_on_connect_error: true,
        retry_on_status: vec![],
        backoff_policy: BackoffPolicy::Exponential,
        max_delay_ms: 10_000,
        jitter: false,
        honor_retry_after: false,
        retry_on_body_failure: false,
    }
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
    stream_body_chunks_capped(resp.bytes_stream(), max_bytes).await
}

async fn stream_body_chunks_capped<S, E>(stream: S, max_bytes: usize) -> Result<(Bytes, bool), E>
where
    S: Stream<Item = Result<Bytes, E>>,
{
    futures::pin_mut!(stream);
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
/// Success resets the passive circuit. Enough consecutive failures open it and,
/// when backoff is configured, establish the recovery-window baseline.
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
                health.last_halfopen.store(process_elapsed_secs(), Ordering::Relaxed);
            }
        }
    }
}

/// Check if a half-open probe should be admitted for passive-only recovery.
///
/// Once the current backoff interval elapses, a one-shot CAS admits at most one
/// caller and advances the recovery-window baseline.
fn should_halfopen_probe(client: &ManagedOutboundClient) -> bool {
    if client.health.healthy.load(Ordering::Relaxed) {
        return false;
    }
    // If an active health task is running it handles recovery; no half-open needed.
    if client.health_task.is_some() {
        return false;
    }
    let Some(passive) = client.passive_health_check.as_ref() else {
        return false;
    };
    let Some(backoff) = &passive.backoff else {
        return false;
    };

    let now = process_elapsed_secs();
    let last = client.health.last_halfopen.load(Ordering::Acquire);
    let interval = client.health.backoff_sec.load(Ordering::Relaxed);

    if halfopen_window_due(now, last, interval)
        && client
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
    false
}

/// Whether the passive-only half-open window is ready for one probe.
fn halfopen_window_due(now: u64, last: u64, interval: u64) -> bool {
    interval == 0 || now >= last.saturating_add(interval)
}

// An elapsed value of zero is a valid timestamp, not a sentinel. Disabled
// backoff is represented by `backoff_sec == 0` in `halfopen_window_due`.
static PROCESS_MONOTONIC_ORIGIN: OnceLock<Instant> = OnceLock::new();

/// Return seconds elapsed from this process's monotonic origin.
fn process_elapsed_secs() -> u64 {
    let origin = PROCESS_MONOTONIC_ORIGIN.get_or_init(Instant::now);
    Instant::now().saturating_duration_since(*origin).as_secs()
}

#[cfg(test)]
#[path = "call_transport_tests.rs"]
mod transport_tests;

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_response_debug_omits_auth_payload() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-private-token", "private-token".parse().unwrap());
        let response = OutboundResponse {
            status: 200,
            headers,
            body: Bytes::from_static(b"private-body"),
        };
        for debug in [format!("{response:?}"), format!("{response:#?}")] {
            assert!(debug.contains("200"));
            assert!(!debug.contains("private"));
        }
        assert_eq!(response.body.as_ref(), b"private-body");
        assert_eq!(response.headers["x-private-token"], "private-token");
    }

    #[test]
    fn outbound_call_error_debug_and_display_are_static_and_detail_free() {
        const CREDENTIAL_MARKER: &str = "credential-marker-must-not-escape";
        let cases = [
            (
                OutboundCallError::Unhealthy,
                "Unhealthy",
                "outbound target is unhealthy",
            ),
            (
                OutboundCallError::RateLimited,
                "RateLimited",
                "outbound rate limit exceeded",
            ),
            (OutboundCallError::Timeout, "Timeout", "outbound request timed out"),
            (OutboundCallError::Connect, "Connect", "outbound connection error"),
            (
                OutboundCallError::ResponseTooLarge,
                "ResponseTooLarge",
                "outbound response body exceeded size cap",
            ),
            (OutboundCallError::Exhausted, "Exhausted", "outbound retries exhausted"),
            (
                OutboundCallError::AuthorityRejected,
                "AuthorityRejected",
                "outbound request rejected: invalid upstream authority",
            ),
        ];

        for (error, expected_debug, expected_display) in cases {
            let debug = format!("{error:?}");
            let display = error.to_string();
            assert_eq!(debug, expected_debug);
            assert_eq!(display, expected_display);
            assert!(!debug.contains(CREDENTIAL_MARKER));
            assert!(!display.contains(CREDENTIAL_MARKER));
        }
    }
    use std::time::Duration;

    use crate::plugins::http::common::http_client::get_http_client_arc;
    use crate::plugins::http::common::outbound::client::{HealthState, ManagedOutboundClient};
    use crate::plugins::http::common::outbound::request::CompiledRequest;
    use crate::plugins::http::common::outbound::target::ResolvedTarget;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
    use edgion_resources::resources::common::request_template::CompiledTemplate;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Compile a template string for test use, panicking on a compile error.
    fn compile_tpl(s: &str) -> CompiledTemplate {
        CompiledTemplate::compile(s).unwrap()
    }

    /// Build a minimal [`ManagedOutboundClient`] with a static target.
    fn make_client(url: &str) -> ManagedOutboundClient {
        ManagedOutboundClient {
            http_client: get_http_client_arc(),
            resolved_target: ResolvedTarget::Static(url.to_string()),
            compiled_request: CompiledRequest::from_spec(&OutboundRequestSpec::default())
                .expect("default spec must compile"),
            health: HealthState::new(),
            rate: None,
            success: None,
            max_response_bytes: 64 * 1024,
            timeout: Duration::from_secs(5),
            timeout_template: None,
            retry: None,
            egress_policy: None,
            passive_health_check: None,
            health_task: None,
            resolved_secrets: None,
        }
    }

    fn non_cloneable_request() -> reqwest::Request {
        let mut request = reqwest::Request::new(
            reqwest::Method::POST,
            reqwest::Url::parse("http://example.com/").unwrap(),
        );
        let body = futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"streaming body")) });
        *request.body_mut() = Some(reqwest::Body::wrap_stream(body));
        assert!(request.try_clone().is_none());
        request
    }

    #[test]
    fn sole_attempt_takes_non_cloneable_request() {
        let mut template = Some(non_cloneable_request());

        assert!(request_for_attempt(&mut template, 1).is_some());
        assert!(template.is_none());
    }

    #[test]
    fn retry_attempt_rejects_non_cloneable_request_without_consuming_template() {
        let mut template = Some(non_cloneable_request());

        assert!(request_for_attempt(&mut template, 2).is_none());
        assert!(template.is_some());
    }

    #[test]
    fn retry_attempt_clones_replayable_request_without_consuming_template() {
        let mut request = reqwest::Request::new(
            reqwest::Method::POST,
            reqwest::Url::parse("http://example.com/").unwrap(),
        );
        *request.body_mut() = Some(Bytes::from_static(b"replayable body").into());
        let mut template = Some(request);

        assert!(request_for_attempt(&mut template, 2).is_some());
        assert!(request_for_attempt(&mut template, 2).is_some());
        assert!(template.is_some());
    }

    #[test]
    fn halfopen_window_due_preserves_zero_interval_and_saturates_boundaries() {
        assert!(halfopen_window_due(4, 5, 0));
        assert!(!halfopen_window_due(4, 5, 1));
        assert!(!halfopen_window_due(9, 5, 5));
        assert!(halfopen_window_due(10, 5, 5));
        assert!(!halfopen_window_due(u64::MAX - 1, u64::MAX - 2, 10));
        assert!(halfopen_window_due(u64::MAX, u64::MAX - 2, 10));
    }

    // ── TDD tests ─────────────────────────────────────────────────────────────

    /// `resolve_timeout` clamps an over-ceiling render to `MAX_OUTBOUND_TIMEOUT_MS`,
    /// tested directly since driving a 60s+ real timeout end-to-end is impractical.
    #[test]
    fn resolve_timeout_clamps_to_ceiling() {
        use edgion_resources::resources::common::external_http::MAX_OUTBOUND_TIMEOUT_MS;

        let mut client = make_client("http://10.0.0.1:1");
        client.timeout = Duration::from_secs(5);
        client.timeout_template = Some(compile_tpl("${header:X-Test-Timeout}"));

        let mut session = MockPluginSession::new();
        session
            .expect_request_header_map()
            .return_const(http::HeaderMap::from_iter([(
                http::HeaderName::from_bytes(b"X-Test-Timeout").unwrap(),
                http::HeaderValue::from_static("999999999"),
            )]));

        let resolved = resolve_timeout(&client, &session);
        assert_eq!(resolved, Duration::from_millis(MAX_OUTBOUND_TIMEOUT_MS));
    }

    /// Sanity: no `timeout_template` set means `resolve_timeout` returns the static
    /// `timeout` unchanged.
    #[test]
    fn resolve_timeout_uses_static_when_no_template() {
        let mut client = make_client("http://10.0.0.1:1");
        client.timeout = Duration::from_secs(7);
        client.timeout_template = None;

        let session = MockPluginSession::new();
        assert_eq!(resolve_timeout(&client, &session), Duration::from_secs(7));
    }

    /// CRITICAL 2 regression: a render below `MIN_OUTBOUND_TIMEOUT_MS` (e.g. "1")
    /// must fall back to the static timeout, not be honored as-is. Without a
    /// floor, a tiny-but-nonzero render would pin the deadline low enough that
    /// the outbound call always times out — a reliable DoS.
    #[test]
    fn resolve_timeout_floor_falls_back_to_static_timeout() {
        let mut client = make_client("http://10.0.0.1:1");
        client.timeout = Duration::from_secs(5);
        client.timeout_template = Some(compile_tpl("${ctx:desired_timeout_ms}"));

        let mut session = MockPluginSession::new();
        session
            .expect_get_ctx_var()
            .withf(|n| n == "desired_timeout_ms")
            .return_const(Some("1".to_string()));

        let resolved = resolve_timeout(&client, &session);
        assert_eq!(resolved, Duration::from_secs(5), "must fall back to the static timeout");
    }

    /// A render exactly at `MIN_OUTBOUND_TIMEOUT_MS` is accepted (the floor is
    /// inclusive), not fallen back.
    #[test]
    fn resolve_timeout_accepts_render_at_floor() {
        use edgion_resources::resources::common::external_http::MIN_OUTBOUND_TIMEOUT_MS;

        let mut client = make_client("http://10.0.0.1:1");
        client.timeout = Duration::from_secs(5);
        client.timeout_template = Some(compile_tpl("${ctx:desired_timeout_ms}"));

        let mut session = MockPluginSession::new();
        session
            .expect_get_ctx_var()
            .withf(|n| n == "desired_timeout_ms")
            .return_const(Some(MIN_OUTBOUND_TIMEOUT_MS.to_string()));

        let resolved = resolve_timeout(&client, &session);
        assert_eq!(resolved, Duration::from_millis(MIN_OUTBOUND_TIMEOUT_MS));
    }

    // ── Backoff tests (Task 1.2) ─────────────────────────────────────────────

    #[test]
    fn backoff_exponential_no_jitter_is_exact() {
        use edgion_resources::resources::common::external_http::{BackoffPolicy, WebhookRetry};
        let retry = WebhookRetry {
            max_retries: 3,
            retry_delay_ms: 100,
            retry_on_timeout: None,
            retry_on_connect_error: true,
            retry_on_status: vec![],
            backoff_policy: BackoffPolicy::Exponential,
            max_delay_ms: 10_000,
            jitter: false,
            honor_retry_after: false,
            retry_on_body_failure: false,
        };
        assert_eq!(compute_retry_delay_ms(1, &retry), 100);
        assert_eq!(compute_retry_delay_ms(2, &retry), 200);
        assert_eq!(compute_retry_delay_ms(3, &retry), 400);
    }

    #[test]
    fn backoff_fixed_is_constant() {
        use edgion_resources::resources::common::external_http::BackoffPolicy;
        let mut retry = crate::plugins::http::common::outbound::call::tests_retry(100);
        retry.backoff_policy = BackoffPolicy::Fixed;
        retry.jitter = false;
        assert_eq!(compute_retry_delay_ms(1, &retry), 100);
        assert_eq!(compute_retry_delay_ms(3, &retry), 100);
    }

    #[test]
    fn backoff_jitter_stays_in_band_and_under_cap() {
        let mut retry = crate::plugins::http::common::outbound::call::tests_retry(1000);
        retry.max_delay_ms = 1000;
        retry.jitter = true;
        let mut all_equal = true;
        let first = compute_retry_delay_ms(5, &retry); // exp would be huge → capped at 1000
        for _ in 0..50 {
            let d = compute_retry_delay_ms(5, &retry);
            assert!((500..=1000).contains(&d), "jittered {d} out of [500,1000]");
            if d != first {
                all_equal = false;
            }
        }
        assert!(!all_equal, "jitter must vary");
    }
}

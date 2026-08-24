//! ForwardAuth plugin implementation
//!
//! Sends the original request's key information (headers, method, URI, etc.) to an
//! external authentication service and decides whether to allow or deny the request
//! based on the auth service's response status code.
//!
//! This is a classic API Gateway authentication pattern, comparable to:
//! - **Traefik**: `forwardAuth` middleware
//! - **nginx**: `auth_request` module
//! - **Kong**: `forward-auth` plugin
//! - **APISIX**: `forward-auth` plugin
//!
//! ## Flow
//!
//! 1. Build auth request with original request metadata (all headers by default)
//! 2. Inject X-Forwarded-Host/Uri/Method/Query so the auth service has full context
//! 3. Send to external auth service via the shared outbound engine
//! 4. If auth service returns 2xx → copy `upstream_headers`, forward to upstream
//! 5. If auth service returns non-2xx → copy `client_headers` + body to client
//! 6. If auth service is unreachable → return `status_on_error`
//!
//! ## Configuration Examples
//!
//! ### Basic: forward all headers
//! ```yaml
//! type: ForwardAuth
//! config:
//!   target:
//!     name: auth-service
//!     port: 8080
//!   decision:
//!     upstreamHeaders:
//!       - X-User-ID
//!       - X-User-Role
//! ```
//!
//! ### Selective: forward specific headers only
//! ```yaml
//! type: ForwardAuth
//! config:
//!   target:
//!     name: auth-service
//!     port: 8443
//!   tls:
//!     verify: true
//!   timeoutMs: 5000
//!   request:
//!     headers:
//!       origin:
//!         - name: Authorization
//!         - name: Cookie
//!   statusOnError: 503
//!   decision:
//!     upstreamHeaders:
//!       - X-User-ID
//!     clientHeaders:
//!       - WWW-Authenticate
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use std::sync::Arc;

use crate::plugins::http::common::auth_common::apply_auth_failure_delay;
use crate::plugins::http::common::outbound::call::outbound_call_with_body;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::common::success::SuccessOutcome;
use edgion_resources::resources::edgion_plugins::{ForwardAuthConfig, ForwardAuthDecision};

/// Maximum bytes of the auth service error body to buffer and return to the client.
///
/// The shared outbound engine enforces `conn.max_response_bytes`; this constant
/// serves as the gateway-wide global cap for forward-auth responses.
const FORWARD_AUTH_GLOBAL_MAX_BYTES: usize = 1024 * 1024; // 1 MiB

pub struct ForwardAuth {
    name: String,
    /// Compiled outbound client: target, TLS, timeout, retry, health, rate limit.
    /// Wrapped in `Arc` so that when the plugin is replaced on hot-reload, the old
    /// client's `Drop` impl aborts the active-health background task.
    client: Arc<ManagedOutboundClient>,
    /// HTTP status code to return when the auth service is unreachable.
    /// Sourced from the shared `ExternalHttpConnection.status_on_error` at
    /// construction time.
    status_on_error: u16,
    /// Decision phase: header propagation, error status, and timing.
    decision: ForwardAuthDecision,
    forward_body: bool,
}

impl ForwardAuth {
    /// Build a [`ForwardAuth`] plugin from the given configuration.
    ///
    /// Returns `Err` if the target, TLS configuration, or request-spec compilation
    /// fails. In normal operation this should not occur because the admission
    /// controller runs `ForwardAuthConfig::validation_error()` before the config
    /// reaches the gateway. Callers may treat an `Err` return as fail-closed (do
    /// not install the plugin).
    pub fn new(config: &ForwardAuthConfig) -> Result<Self, String> {
        let client = ManagedOutboundClient::from_parts(
            &config.conn,
            &config.request,
            config.resolved_secrets.as_ref(),
            "ForwardAuth",
            // Auth checks are idempotent GET/HEAD-style calls: retry a timed-out
            // request by default.
            true,
        )?;
        Ok(ForwardAuth {
            name: "ForwardAuth".to_string(),
            client: Arc::new(client),
            status_on_error: config.conn.status_on_error,
            decision: config.decision.clone(),
            forward_body: config.forward_body,
        })
    }

    /// Write an error response directly to the client and return `ErrTerminateRequest`.
    ///
    /// Sets the caller-provided `Content-Type` when present and `Connection: close`,
    /// then appends any `extra_headers` (the `client_headers` harvested from the
    /// auth service response on an auth-failure path).
    async fn send_error_response(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        status: u16,
        body: &[u8],
        content_type: Option<&str>,
        extra_headers: Option<&[(String, String)]>,
    ) -> PluginRunningResult {
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };

        if let Some(content_type) = content_type {
            let _ = resp.insert_header("Content-Type", content_type);
        }
        let _ = resp.insert_header("Connection", "close");

        // Add extra headers (e.g., client_headers from auth service response).
        // The values were validated by HeaderValue::to_str() upstream, so any
        // insert_header failure here indicates a non-token header name in the
        // operator's `client_headers` config.
        if let Some(headers) = extra_headers {
            for (name, value) in headers {
                if resp.insert_header(name.clone(), value.clone()).is_err() {
                    plugin_log.push_err("invalid response header");
                }
            }
        }

        let _ = session.write_response_header(Box::new(resp), false).await;
        let _ = session
            .write_response_body(Some(Bytes::from(body.to_vec())), true)
            .await;
        session.shutdown().await;
        PluginRunningResult::ErrTerminateRequest
    }
}

#[async_trait]
impl RequestFilter for ForwardAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Build the X-Forwarded-* context headers so the auth service knows which
        // host, path, method, and query the original client request targeted.
        // These are injected as extra_headers into the outbound request rather than
        // compiled into the request spec, because they are per-request dynamic values
        // (not static config-time constants).
        let host = session.header_value("host").unwrap_or_default();
        let path = session.get_path().to_string();
        let method = session.get_method().to_string();
        let query = session.get_query().unwrap_or_default();

        let extra_headers: &[(&str, &str)] = &[
            ("X-Forwarded-Host", host.as_str()),
            ("X-Forwarded-Uri", path.as_str()),
            ("X-Forwarded-Method", method.as_str()),
            ("X-Forwarded-Query", query.as_str()),
        ];

        // Call the shared outbound engine:
        //   health gate → rate gate → compile+build request → retry loop →
        //   passive-health tracking → response-size cap →
        //   Ok(OutboundResponse) for any completed HTTP exchange.
        let body = if self.forward_body {
            match session.read_request_body(None).await {
                Ok(body) => Some(body),
                Err(_) => {
                    session.record_error(edgion_resources::EdgionErrorCode::BodyReadAborted);
                    plugin_log.push_err("forward auth body unavailable");
                    return PluginRunningResult::ErrResponse {
                        status: 500,
                        body: None,
                    };
                }
            }
        } else {
            None
        };

        let resp = match outbound_call_with_body(
            &self.client,
            session,
            None,
            FORWARD_AUTH_GLOBAL_MAX_BYTES,
            extra_headers,
            body,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                // Transport-level failure: connection refused, DNS, timeout, rate/health gate.
                // Surface in access-log diagnostics for gateway-level visibility; PluginLog
                // captures the per-request plugin-side detail.
                session.record_error(edgion_resources::EdgionErrorCode::ForwardAuthServiceUnreachable);
                plugin_log.push_fmt("upstream unreachable {}", la![e.to_string().as_str()]);
                // Record BEFORE `send_error_response`'s self-write
                // (`write_response_header`) so the queued dye header op is
                // drained by that write.
                session.record_outcome(DyeOutcome::FailClose, "auth-server-unreachable");
                return self
                    .send_error_response(
                        session,
                        plugin_log,
                        self.status_on_error,
                        b"{\"message\":\"Auth service unavailable\"}",
                        Some("application/json"),
                        None,
                    )
                    .await;
            }
        };

        let outcome = self.client.classify_detail(&resp);
        if outcome == SuccessOutcome::Success {
            // Auth passed: copy specified response headers to the upstream request.
            for header_name in &self.decision.upstream_headers {
                if let Some(value) = resp.headers.get(header_name.as_str()) {
                    if let Ok(v) = value.to_str() {
                        let _ = session.set_request_header(header_name, v);
                    }
                }
            }
            // Optionally strip credentials so they are not forwarded upstream.
            if self.decision.hide_credentials {
                let _ = session.remove_request_header("authorization");
            }
            plugin_log.push_fmt("auth pass {}", la![resp.status.to_string().as_str()]);
            PluginRunningResult::GoodNext
        } else {
            // Auth failed: collect client_headers from auth response for the error reply.
            let extra: Vec<(String, String)> = self
                .decision
                .client_headers
                .iter()
                .filter_map(|hn| {
                    resp.headers
                        .get(hn.as_str())
                        .and_then(|v| v.to_str().ok())
                        .map(|v| (hn.clone(), v.to_string()))
                })
                .collect();

            // `StatusFail` is a genuine non-2xx from the auth service: relay it
            // unchanged. `BodyFail` means the auth service answered with an
            // otherwise-accepted status (e.g. 200) but the `success.body`
            // predicate rejected it — relaying that status verbatim would tell
            // the client "200 OK" for a denied request, so force 403 instead.
            let client_status = if outcome == SuccessOutcome::BodyFail {
                http::StatusCode::FORBIDDEN.as_u16()
            } else {
                resp.status
            };

            plugin_log.push_fmt(
                "auth deny upstream_status={} client_status={} reason={}",
                la![
                    resp.status.to_string().as_str(),
                    client_status.to_string().as_str(),
                    if outcome == SuccessOutcome::BodyFail {
                        "body_predicate_failed"
                    } else {
                        "status_rejected"
                    }
                ],
            );
            // Apply anti-brute-force delay before responding to the client.
            apply_auth_failure_delay(self.decision.auth_failure_delay_ms).await;
            let content_type = resp
                .headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok());
            self.send_error_response(
                session,
                plugin_log,
                client_status,
                &resp.body,
                content_type,
                Some(&extra),
            )
            .await
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
    use crate::plugins::runtime::log::PluginLog;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::common::success::{BodyPredicate, PredicateOp, SuccessSpec};
    use edgion_resources::resources::edgion_plugins::ForwardAuthDecision;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Remap a wiremock server URI from `http://127.0.0.1:PORT` to
    /// `http://localhost:PORT` so that `dial_url`'s SSRF guard does not block it.
    ///
    /// `127.0.0.1` is a loopback IP and `validate_endpoint_backend_addr` rejects
    /// it. `localhost` is a DNS name — `extract_ip_literal` cannot parse it as a
    /// `SocketAddr`, so the literal-level SSRF check is bypassed, and reqwest
    /// resolves it to the mock server normally.
    fn localhost_uri(server_uri: &str) -> String {
        server_uri.replace("127.0.0.1", "localhost")
    }

    /// Build a `ForwardAuth` plugin backed by a `ManagedOutboundClient` pointed
    /// at `url`.  Uses a `Static` resolved target and a default (no-forward-all)
    /// compiled request; no health task is spawned.
    fn make_forward_auth(url: &str, decision: ForwardAuthDecision) -> ForwardAuth {
        let client = ManagedOutboundClient::test_with_static_target(url);
        ForwardAuth {
            name: "ForwardAuth".to_string(),
            client: Arc::new(client),
            status_on_error: 503,
            decision,
            forward_body: false,
        }
    }

    /// Same as `make_forward_auth`, but with a `success` spec installed on the
    /// client so `classify_detail` can distinguish `StatusFail` from `BodyFail`.
    fn make_forward_auth_with_success(url: &str, decision: ForwardAuthDecision, success: SuccessSpec) -> ForwardAuth {
        let mut client = ManagedOutboundClient::test_with_static_target(url);
        client.success = Some(success);
        ForwardAuth {
            name: "ForwardAuth".to_string(),
            client: Arc::new(client),
            status_on_error: 503,
            decision,
            forward_body: false,
        }
    }

    /// Build a `MockPluginSession` with the minimal expectations needed by
    /// `ForwardAuth::run_request`:
    ///
    /// - `header_value("host")` → `"example.com"`
    /// - `get_path()` → `"/api/data"`
    /// - `get_method()` → `"GET"` (called twice: once for X-Forwarded-Method,
    ///   once inside `build_request` for method resolution)
    /// - `get_query()` → `None`
    /// - `get_request_path()` → `"/check"` (used by `build_request` for path)
    fn make_session() -> MockPluginSession {
        let mut s = MockPluginSession::new();
        s.expect_header_value()
            .withf(|name| name == "host")
            .return_const(Some("example.com".to_string()));
        s.expect_get_path().return_const("/api/data".to_string());
        s.expect_get_method().return_const("GET".to_string());
        s.expect_get_query().return_const(None::<String>);
        s.expect_get_request_path().return_const("/check".to_string());
        s
    }

    // ── TDD tests ─────────────────────────────────────────────────────────────

    /// P0 TDD: a 200 auth-service response causes the listed `upstream_headers`
    /// to be copied from the auth response onto the upstream request, then returns
    /// `GoodNext`.
    #[tokio::test]
    async fn pass_copies_upstream_headers() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("x-user-id", "user123")
                    .set_body_bytes(b"ok".to_vec()),
            )
            .mount(&server)
            .await;

        let decision = ForwardAuthDecision {
            upstream_headers: vec!["x-user-id".to_string()],
            ..ForwardAuthDecision::default()
        };
        let fa = make_forward_auth(&localhost_uri(&server.uri()), decision);

        let mut session = make_session();
        // Expect the upstream header to be set on the inbound request.
        session
            .expect_set_request_header()
            .withf(|name, value| name == "x-user-id" && value == "user123")
            .times(1)
            .returning(|_, _| Ok(()));

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.run_request(&mut session, &mut log).await;
        assert!(
            matches!(result, PluginRunningResult::GoodNext),
            "expected GoodNext on auth pass, got {result:?}"
        );
    }

    /// P0 TDD: a non-2xx auth-service response copies the listed `client_headers`
    /// to the error response sent to the client, and returns `ErrTerminateRequest`.
    #[tokio::test]
    async fn deny_copies_client_headers_and_shortcircuits() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .respond_with(
                ResponseTemplate::new(401)
                    .append_header("www-authenticate", "Bearer")
                    .insert_header("content-type", "text/plain; charset=utf-8")
                    .set_body_bytes(b"no".to_vec()),
            )
            .mount(&server)
            .await;

        let decision = ForwardAuthDecision {
            client_headers: vec!["www-authenticate".to_string()],
            ..ForwardAuthDecision::default()
        };
        let fa = make_forward_auth(&localhost_uri(&server.uri()), decision);

        let mut session = make_session();
        // The error response is written to the session via the standard Pingora path.
        // Regression: a genuine StatusFail (auth service's own non-2xx) must be
        // relayed to the client UNCHANGED — never overridden to 403.
        session
            .expect_write_response_header()
            .withf(|resp, _| {
                resp.status.as_u16() == 401
                    && resp.headers.get("content-type").and_then(|v| v.to_str().ok())
                        == Some("text/plain; charset=utf-8")
            })
            .returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.run_request(&mut session, &mut log).await;
        assert!(
            matches!(result, PluginRunningResult::ErrTerminateRequest),
            "expected ErrTerminateRequest on auth deny, got {result:?}"
        );
        // Verify the plugin log recorded the deny.
        assert!(
            log.contains("auth deny"),
            "expected plugin log to mention auth deny; got: {log:?}"
        );
    }

    /// P0 TDD: the auth service returns `200` (an otherwise-accepted status) but
    /// the configured `success.body` predicate FAILS on the body — this is a
    /// `BodyFail`, not a `StatusFail`. The client-facing status must be
    /// overridden to 403 (NOT relayed as the auth service's 200), so a client
    /// keying off the status code cannot mistake a deny for a success.
    #[tokio::test]
    async fn body_predicate_denial_overrides_status_to_403() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_bytes(br#"{"allowed":false}"#.to_vec()),
            )
            .mount(&server)
            .await;

        let success = SuccessSpec {
            status_codes: None,
            body: vec![BodyPredicate {
                pointer: "/allowed".to_string(),
                op: PredicateOp::Equals(serde_json::Value::Bool(true)),
            }],
        };
        let fa = make_forward_auth_with_success(&localhost_uri(&server.uri()), ForwardAuthDecision::default(), success);

        let mut session = make_session();
        // The client must see 403, NOT the auth service's own 200.
        session
            .expect_write_response_header()
            .withf(|resp, _| resp.status.as_u16() == 403)
            .returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.run_request(&mut session, &mut log).await;
        assert!(
            matches!(result, PluginRunningResult::ErrTerminateRequest),
            "expected ErrTerminateRequest on body-predicate deny, got {result:?}"
        );
        assert!(
            log.contains("auth deny"),
            "expected plugin log to mention auth deny; got: {log:?}"
        );
    }

    /// Task 9: when the auth service is unreachable, the fail-close path
    /// records `DyeOutcome::FailClose` with cause `auth-server-unreachable`
    /// BEFORE `send_error_response`'s self-write (`write_response_header`) —
    /// enforced here via a mockall `Sequence` so an ordering regression fails
    /// the test, not just a missing-call assertion.
    #[tokio::test]
    async fn transport_fail_records_fail_close_before_response_write() {
        use mockall::Sequence;
        use std::sync::atomic::Ordering;

        let fa = {
            let client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
            client.health.healthy.store(false, Ordering::Relaxed);

            ForwardAuth {
                name: "ForwardAuth".to_string(),
                client: Arc::new(client),
                status_on_error: 503,
                decision: ForwardAuthDecision::default(),
                forward_body: false,
            }
        };

        let mut session = make_session();
        session
            .expect_record_error()
            .withf(|status| matches!(status, edgion_resources::EdgionErrorCode::ForwardAuthServiceUnreachable))
            .times(1)
            .returning(|_| ());

        let mut seq = Sequence::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "auth-server-unreachable")
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());
        session
            .expect_write_response_header()
            .withf(|resp, _| resp.headers.get("content-type").and_then(|v| v.to_str().ok()) == Some("application/json"))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.run_request(&mut session, &mut log).await;
        assert!(
            matches!(result, PluginRunningResult::ErrTerminateRequest),
            "expected ErrTerminateRequest on transport failure, got {result:?}"
        );
    }

    /// Task 9: an ordinary auth-server DENY (401/403) is an upstream
    /// decision, not a dependency failure — `record_outcome` must NOT be
    /// called. No `expect_record_outcome` is installed, so any call panics
    /// the mock; this pins the negative case explicitly rather than relying
    /// on the pre-existing deny tests' silence.
    #[tokio::test]
    async fn auth_deny_does_not_record_outcome() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(403).set_body_bytes(b"no".to_vec()))
            .mount(&server)
            .await;

        let fa = make_forward_auth(&localhost_uri(&server.uri()), ForwardAuthDecision::default());

        let mut session = make_session();
        session
            .expect_write_response_header()
            .withf(|resp, _| resp.status.as_u16() == 403)
            .returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.run_request(&mut session, &mut log).await;
        assert!(
            matches!(result, PluginRunningResult::ErrTerminateRequest),
            "expected ErrTerminateRequest on auth deny, got {result:?}"
        );
    }
}

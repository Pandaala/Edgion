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
//! 6. If auth service is unreachable → return `status_on_error` or degrade
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
//!   decision:
//!     upstreamHeaders:
//!       - X-User-ID
//!     clientHeaders:
//!       - WWW-Authenticate
//!     allowDegradation: true
//!     statusOnError: 503
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use std::sync::Arc;

use crate::plugins::http::common::auth_common::apply_auth_failure_delay;
use crate::plugins::http::common::outbound::call::outbound_call;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
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
    /// Decision phase: header propagation, degradation, error status, and timing.
    decision: ForwardAuthDecision,
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
        )?;
        Ok(ForwardAuth {
            name: "ForwardAuth".to_string(),
            client: Arc::new(client),
            decision: config.decision.clone(),
        })
    }

    /// Write an error response directly to the client and return `ErrTerminateRequest`.
    ///
    /// Sets `Content-Type: application/json` and `Connection: close`, then appends
    /// any `extra_headers` (the `client_headers` harvested from the auth service
    /// response on an auth-failure path).
    async fn send_error_response(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        status: u16,
        body: &[u8],
        extra_headers: Option<&[(String, String)]>,
    ) -> PluginRunningResult {
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };

        let _ = resp.insert_header("Content-Type", "application/json");
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

    /// Check whether the auth response status code indicates success.
    ///
    /// Uses `conn.success_status_codes` when configured; falls back to the
    /// standard 2xx range (200–299).
    fn is_success_status(&self, status: u16) -> bool {
        match &self.client.success_status_codes {
            Some(codes) => codes.contains(&status),
            None => (200u16..300u16).contains(&status),
        }
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
        let resp = match outbound_call(
            &self.client,
            session,
            None,
            FORWARD_AUTH_GLOBAL_MAX_BYTES,
            extra_headers,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                // Transport-level failure: connection refused, DNS, timeout, rate/health gate.
                // Surface in the access log `errors` for gateway-level visibility; PluginLog
                // captures the per-request plugin-side detail.
                session.add_error(edgion_resources::EdgionStatus::ForwardAuthServiceUnreachable);
                if self.decision.allow_degradation {
                    plugin_log.push_err("upstream unreachable:degraded");
                    return PluginRunningResult::GoodNext;
                }
                plugin_log.push_fmt("upstream unreachable {}", la![e.to_string().as_str()]);
                return self
                    .send_error_response(
                        session,
                        plugin_log,
                        self.decision.status_on_error,
                        b"{\"message\":\"Auth service unavailable\"}",
                        None,
                    )
                    .await;
            }
        };

        if self.is_success_status(resp.status) {
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

            // The body was already size-capped by the outbound engine; pass it through.
            let body = resp.body;

            plugin_log.push_fmt("auth deny {}", la![resp.status.to_string().as_str()]);
            // Apply anti-brute-force delay before responding to the client.
            apply_auth_failure_delay(self.decision.auth_failure_delay_ms).await;
            self.send_error_response(session, plugin_log, resp.status, &body, Some(&extra))
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
    use edgion_resources::resources::edgion_plugins::ForwardAuthDecision;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Remap a wiremock server URI from `http://127.0.0.1:PORT` to
    /// `http://localhost:PORT` so that `dial_url`'s SSRF guard does not block it.
    ///
    /// `127.0.0.1` is a loopback IP and `validate_endpoint_backend_addr` rejects
    /// it. `localhost` is a DNS name — `extract_socket_addr` cannot parse it as a
    /// `SocketAddr`, so the SSRF check is bypassed, and reqwest resolves it to the
    /// mock server normally.
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
            decision,
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
        session.expect_write_response_header().returning(|_, _| Ok(()));
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

    /// P0 TDD: when the auth service is unreachable and `allow_degradation` is
    /// true, the plugin returns `GoodNext` (fail-open / degraded mode).
    #[tokio::test]
    async fn transport_fail_degrades_when_allowed() {
        use std::sync::atomic::Ordering;

        // Use a routable (non-loopback) IP so the SSRF guard passes, but mark the
        // client unhealthy so `outbound_call` immediately returns `Err(Unhealthy)`
        // without touching the network.
        let fa = {
            let client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
            // Force unhealthy state so `outbound_call` returns Err(Unhealthy) instantly.
            client.health.healthy.store(false, Ordering::Relaxed);

            ForwardAuth {
                name: "ForwardAuth".to_string(),
                client: Arc::new(client),
                decision: ForwardAuthDecision {
                    allow_degradation: true,
                    ..ForwardAuthDecision::default()
                },
            }
        };

        let mut session = make_session();
        // add_error is called to record the unreachable condition in the access log.
        session
            .expect_add_error()
            .withf(|status| matches!(status, edgion_resources::EdgionStatus::ForwardAuthServiceUnreachable))
            .times(1)
            .returning(|_| ());

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.run_request(&mut session, &mut log).await;
        assert!(
            matches!(result, PluginRunningResult::GoodNext),
            "expected GoodNext on degraded transport failure, got {result:?}"
        );
        assert!(
            log.contains("degraded"),
            "expected plugin log to mention degraded; got: {log:?}"
        );
    }
}

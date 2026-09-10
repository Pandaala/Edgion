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
use crate::plugins::http::common::outbound::call::{outbound_call_with_body, OutboundResponse};
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::common::success::SuccessOutcome;
use edgion_resources::resources::edgion_plugins::{ForwardAuthConfig, ForwardAuthDecision};

/// Maximum bytes of the auth service error body to buffer and return to the client.
///
/// The shared outbound engine enforces `conn.max_response_bytes`; this constant
/// serves as the gateway-wide global cap for forward-auth responses.
const FORWARD_AUTH_GLOBAL_MAX_BYTES: usize = 1024 * 1024; // 1 MiB

pub struct ForwardAuth {
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
            client: Arc::new(client),
            status_on_error: config.conn.status_on_error,
            decision: config.decision.clone(),
            forward_body: config.forward_body,
        })
    }

    /// Write an error response directly to the client and return `ErrTerminateRequest`.
    ///
    /// Sets the caller-provided `Content-Type` when present and `Connection: close`,
    /// then copies configured `client_headers` from the auth service response.
    async fn send_error_response(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        status: u16,
        body: Bytes,
        content_type: Option<&str>,
        forwarded_headers: Option<(&reqwest::header::HeaderMap, &[String])>,
    ) -> PluginRunningResult {
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };

        if let Some(content_type) = content_type {
            let _ = resp.insert_header("Content-Type", content_type);
        }
        let _ = resp.insert_header("Connection", "close");

        // Add configured client_headers from the auth service response in
        // config order. HeaderMap::get intentionally preserves the existing
        // first-value/replace behavior for repeated response fields.
        if let Some((auth_headers, client_headers)) = forwarded_headers {
            for name in client_headers {
                if let Some(value) = auth_headers.get(name.as_str()).and_then(|value| value.to_str().ok()) {
                    // ResponseHeader's borrowed-name conversion is restricted
                    // to static strings; keep the configured name owned at the
                    // insertion boundary while borrowing the auth value.
                    if resp.insert_header(name.clone(), value).is_err() {
                        plugin_log.push_err("invalid response header");
                    }
                }
            }
        }

        let _ = session.write_response_header(Box::new(resp), false).await;
        let _ = session.write_response_body(Some(body), true).await;
        session.shutdown().await;
        PluginRunningResult::ErrTerminateRequest
    }

    async fn handle_outbound_response(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        response: OutboundResponse,
    ) -> PluginRunningResult {
        let outcome = self.client.classify_detail(&response);
        if outcome == SuccessOutcome::Success {
            // Auth passed: copy specified response headers to the upstream request.
            for header_name in &self.decision.upstream_headers {
                if let Some(value) = response.headers.get(header_name.as_str()) {
                    if let Ok(v) = value.to_str() {
                        let _ = session.set_request_header(header_name, v);
                    }
                }
            }
            // Optionally strip credentials so they are not forwarded upstream.
            if self.decision.hide_credentials {
                let _ = session.remove_request_header("authorization");
            }
            plugin_log.push_fmt("auth pass {}", la![response.status.to_string().as_str()]);
            return PluginRunningResult::GoodNext;
        }

        // `StatusFail` is a genuine non-2xx from the auth service: relay it
        // unchanged. `BodyFail` means the auth service answered with an
        // otherwise-accepted status (e.g. 200) but the `success.body`
        // predicate rejected it — relaying that status verbatim would tell
        // the client "200 OK" for a denied request, so force 403 instead.
        let client_status = if outcome == SuccessOutcome::BodyFail {
            http::StatusCode::FORBIDDEN.as_u16()
        } else {
            response.status
        };

        plugin_log.push_fmt(
            "auth deny upstream_status={} client_status={} reason={}",
            la![
                response.status.to_string().as_str(),
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

        let OutboundResponse { headers, body, .. } = response;
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        self.send_error_response(
            session,
            plugin_log,
            client_status,
            body,
            content_type,
            Some((&headers, &self.decision.client_headers)),
        )
        .await
    }
}

#[async_trait]
impl RequestFilter for ForwardAuth {
    fn name(&self) -> &str {
        "ForwardAuth"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
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

        // Build the X-Forwarded-* context headers so the auth service knows which
        // host, path, method, and query the original client request targeted.
        // These are injected as extra_headers into the outbound request rather than
        // compiled into the request spec, because they are per-request dynamic values
        // (not static config-time constants).
        let host = session.header_str("host").unwrap_or_default();
        let path = session.get_path();
        let method = session.get_method();
        let query = session.get_query().unwrap_or_default();

        let extra_headers: &[(&str, &str)] = &[
            ("X-Forwarded-Host", host),
            ("X-Forwarded-Uri", path),
            ("X-Forwarded-Method", method),
            ("X-Forwarded-Query", query.as_str()),
        ];

        // Call the shared outbound engine:
        //   health gate → rate gate → compile+build request → retry loop →
        //   passive-health tracking → response-size cap →
        //   Ok(OutboundResponse) for any completed HTTP exchange.
        let mut diagnostics = crate::plugins::http::common::outbound::request::OutboundRequestDiagnostics::default();
        let result = outbound_call_with_body(
            &self.client,
            session,
            None,
            FORWARD_AUTH_GLOBAL_MAX_BYTES,
            extra_headers,
            body,
            &mut diagnostics,
        )
        .await;
        diagnostics.emit(plugin_log);
        let resp = match result {
            Ok(r) => r,
            Err(_) => {
                // Transport-level failure: connection refused, DNS, timeout, rate/health gate.
                // Surface in access-log diagnostics for gateway-level visibility; PluginLog
                // captures the per-request plugin-side detail.
                session.record_error(edgion_resources::EdgionErrorCode::ForwardAuthServiceUnreachable);
                plugin_log.push_err("upstream unreachable");
                // Record BEFORE `send_error_response`'s self-write
                // (`write_response_header`) so the queued dye header op is
                // drained by that write.
                session.record_outcome(DyeOutcome::FailClose, "auth-server-unreachable");
                return self
                    .send_error_response(
                        session,
                        plugin_log,
                        self.status_on_error,
                        Bytes::from_static(b"{\"message\":\"Auth service unavailable\"}"),
                        Some("application/json"),
                        None,
                    )
                    .await;
            }
        };
        self.handle_outbound_response(session, plugin_log, resp).await
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
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use edgion_resources::resources::common::success::{BodyPredicate, PredicateOp, SuccessSpec};
    use edgion_resources::resources::edgion_plugins::ForwardAuthDecision;
    use mockall::Sequence;
    use reqwest::header::{HeaderMap, HeaderValue};

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_forward_auth(decision: ForwardAuthDecision) -> ForwardAuth {
        let client = ManagedOutboundClient::test_with_static_target("http://example.com");
        ForwardAuth {
            client: Arc::new(client),
            status_on_error: 503,
            decision,
            forward_body: false,
        }
    }

    fn make_forward_auth_with_success(decision: ForwardAuthDecision, success: SuccessSpec) -> ForwardAuth {
        let mut client = ManagedOutboundClient::test_with_static_target("http://example.com");
        client.success = Some(success);
        ForwardAuth {
            client: Arc::new(client),
            status_on_error: 503,
            decision,
            forward_body: false,
        }
    }

    fn make_request_session() -> MockPluginSession {
        let mut s = MockPluginSession::new();
        setup_header_mocks(&mut s, &[("host", "example.com")]);
        s.expect_get_path().return_const("/api/data".to_string());
        s.expect_get_method().return_const("GET".to_string());
        s.expect_get_query().return_const(None::<String>);
        s
    }

    fn response(status: u16, headers: HeaderMap, body: Bytes) -> OutboundResponse {
        OutboundResponse { status, headers, body }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn name_is_static_across_instances() {
        let first = make_forward_auth(ForwardAuthDecision::default());
        let second = make_forward_auth(ForwardAuthDecision::default());

        assert_eq!(first.name(), "ForwardAuth");
        assert_eq!(second.name(), "ForwardAuth");
        assert!(std::ptr::eq(first.name(), second.name()));
    }

    #[tokio::test]
    async fn pass_copies_upstream_headers() {
        let decision = ForwardAuthDecision {
            upstream_headers: vec!["x-user-id".to_string()],
            hide_credentials: true,
            ..ForwardAuthDecision::default()
        };
        let fa = make_forward_auth(decision);

        let mut headers = HeaderMap::new();
        headers.append("x-user-id", HeaderValue::from_static("user123"));
        headers.append("x-user-id", HeaderValue::from_static("ignored-second-value"));
        let outbound = response(200, headers, Bytes::from_static(b"ok"));

        let mut session = MockPluginSession::new();
        let mut seq = Sequence::new();
        session
            .expect_set_request_header()
            .withf(|name, value| name == "x-user-id" && value == "user123")
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_request_header()
            .withf(|name| name == "authorization")
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Ok(()));

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

        assert!(matches!(result, PluginRunningResult::GoodNext));
        assert_eq!(log.render_for_test(), "auth pass 200");
    }

    #[tokio::test]
    async fn deny_copies_client_headers_and_shortcircuits() {
        let decision = ForwardAuthDecision {
            client_headers: vec!["x-auth-reason".to_string(), "www-authenticate".to_string()],
            ..ForwardAuthDecision::default()
        };
        let fa = make_forward_auth(decision);

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("text/plain; charset=utf-8"));
        headers.insert("x-auth-reason", HeaderValue::from_static("expired"));
        headers.append("www-authenticate", HeaderValue::from_static("Bearer"));
        headers.append("www-authenticate", HeaderValue::from_static("ignored-second-value"));
        let body = Bytes::from(Vec::from(&b"denied-body"[..]));
        let body_ptr = body.as_ptr() as usize;
        let body_len = body.len();
        let outbound = response(401, headers, body);

        let mut session = MockPluginSession::new();
        let mut seq = Sequence::new();
        session
            .expect_write_response_header()
            .withf(|resp, _| {
                resp.status.as_u16() == 401
                    && resp.headers.get("content-type").and_then(|v| v.to_str().ok())
                        == Some("text/plain; charset=utf-8")
                    && resp.headers.get("connection").and_then(|v| v.to_str().ok()) == Some("close")
                    && resp.headers.get("x-auth-reason").and_then(|v| v.to_str().ok()) == Some("expired")
                    && resp.headers.get("www-authenticate").and_then(|v| v.to_str().ok()) == Some("Bearer")
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(move |body, end_of_stream| {
                *end_of_stream
                    && body.as_ref().is_some_and(|body| {
                        body.as_ptr() as usize == body_ptr && body.len() == body_len && body.as_ref() == b"denied-body"
                    })
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

        assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
        assert_eq!(
            log.render_for_test(),
            "auth deny upstream_status=401 client_status=401 reason=status_rejected"
        );
    }

    #[tokio::test]
    async fn body_predicate_denial_overrides_status_to_403() {
        let success = SuccessSpec {
            status_codes: None,
            body: vec![BodyPredicate {
                pointer: "/allowed".to_string(),
                op: PredicateOp::Equals(serde_json::Value::Bool(true)),
            }],
        };
        let fa = make_forward_auth_with_success(ForwardAuthDecision::default(), success);

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let outbound = response(200, headers, Bytes::from_static(br#"{"allowed":false}"#));

        let mut session = MockPluginSession::new();
        let mut seq = Sequence::new();
        session
            .expect_write_response_header()
            .withf(|resp, _| {
                resp.status.as_u16() == 403
                    && resp.headers.get("content-type").and_then(|value| value.to_str().ok())
                        == Some("application/json")
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(|body, end_of_stream| *end_of_stream && body.as_deref() == Some(br#"{"allowed":false}"#.as_slice()))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

        assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
        assert_eq!(
            log.render_for_test(),
            "auth deny upstream_status=200 client_status=403 reason=body_predicate_failed"
        );
    }

    #[tokio::test]
    async fn transport_fail_records_fail_close_before_response_write() {
        use std::sync::atomic::Ordering;

        let fa = {
            let client = ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080");
            client.health.healthy.store(false, Ordering::Relaxed);

            ForwardAuth {
                client: Arc::new(client),
                status_on_error: 503,
                decision: ForwardAuthDecision::default(),
                forward_body: false,
            }
        };

        let mut session = make_request_session();
        let mut seq = Sequence::new();
        session
            .expect_record_error()
            .withf(|status| matches!(status, edgion_resources::EdgionErrorCode::ForwardAuthServiceUnreachable))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| ());
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "auth-server-unreachable")
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());
        session
            .expect_write_response_header()
            .withf(|resp, _| {
                resp.status.as_u16() == 503
                    && resp.headers.get("content-type").and_then(|v| v.to_str().ok()) == Some("application/json")
                    && resp.headers.get("connection").and_then(|v| v.to_str().ok()) == Some("close")
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(|body, end_of_stream| {
                *end_of_stream && body.as_deref() == Some(b"{\"message\":\"Auth service unavailable\"}".as_slice())
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.run_request(&mut session, &mut log).await;

        assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
        assert_eq!(log.render_for_test(), "err: upstream unreachable");
    }

    #[tokio::test]
    async fn presence_warning_reaches_plugin_log_without_changing_fail_close() {
        use crate::plugins::http::common::outbound::request::test_presence_failure_request;
        use edgion_resources::resources::common::outbound_request::PresencePolicy;

        for policy in [
            PresencePolicy::WarnIfMissing,
            PresencePolicy::SkipIfMissing,
            PresencePolicy::Required,
        ] {
            let mut client = ManagedOutboundClient::test_with_static_target("http://example.test");
            client.compiled_request = test_presence_failure_request(policy);
            let fa = ForwardAuth {
                client: Arc::new(client),
                status_on_error: 503,
                decision: ForwardAuthDecision::default(),
                forward_body: false,
            };
            let mut session = make_request_session();
            session.expect_get_cookie().return_const(None::<String>);
            session.expect_record_error().times(1).return_const(());
            session
                .expect_record_outcome()
                .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "auth-server-unreachable")
                .times(1)
                .return_const(());
            session
                .expect_write_response_header()
                .withf(|response, _| response.status.as_u16() == 503)
                .times(1)
                .returning(|_, _| Ok(()));
            session
                .expect_write_response_body()
                .withf(|body, eos| {
                    *eos && body.as_deref() == Some(b"{\"message\":\"Auth service unavailable\"}".as_slice())
                })
                .times(1)
                .returning(|_, _| Ok(()));
            session.expect_shutdown().times(1).return_const(());
            let mut log = PluginLog::new("ForwardAuth");
            assert!(matches!(
                fa.run_request(&mut session, &mut log).await,
                PluginRunningResult::ErrTerminateRequest
            ));
            let rendered = log.render_for_test();
            assert_eq!(
                rendered.matches("warn outbound field missing").count(),
                usize::from(matches!(policy, PresencePolicy::WarnIfMissing))
            );
            assert!(rendered.ends_with("err: upstream unreachable"));
            assert!(!rendered.contains("private"));
        }
    }

    #[tokio::test]
    async fn auth_deny_does_not_record_outcome() {
        let fa = make_forward_auth(ForwardAuthDecision::default());
        let outbound = response(403, HeaderMap::new(), Bytes::from_static(b"no"));

        let mut session = MockPluginSession::new();
        let mut seq = Sequence::new();
        session
            .expect_write_response_header()
            .withf(|resp, _| resp.status.as_u16() == 403)
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_write_response_body()
            .withf(|body, end_of_stream| *end_of_stream && body.as_deref() == Some(b"no".as_slice()))
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        session
            .expect_shutdown()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|| {});

        let mut log = PluginLog::new("ForwardAuth");
        let result = fa.handle_outbound_response(&mut session, &mut log, outbound).await;

        assert!(matches!(result, PluginRunningResult::ErrTerminateRequest));
        assert_eq!(
            log.render_for_test(),
            "auth deny upstream_status=403 client_status=403 reason=status_rejected"
        );
    }
}

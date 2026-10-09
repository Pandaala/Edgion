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
//!       forwardAll: false
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
use crate::plugins::http::common::connection_listed_hop_by_hop;
use crate::plugins::http::common::outbound::call::{outbound_call_with_body, OutboundResponse};
use crate::plugins::http::common::outbound::client::{ManagedOutboundClient, PreparedManagedHttp};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::common::success::SuccessOutcome;
use edgion_resources::resources::edgion_plugins::{ForwardAuthConfig, ForwardAuthDecision};
use edgion_resources::response_header_policy::is_protected_response_header;

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
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        let client = ManagedOutboundClient::from_parts(
            &config.conn,
            &config.request,
            config.resolved_secrets.as_ref(),
            "ForwardAuth",
            // Auth checks are idempotent GET/HEAD-style calls: retry a timed-out
            // request by default.
            true,
        )?;
        Ok(Self::from_client(config, client))
    }

    pub(crate) fn from_owned(input: &PreparedManagedHttp<'_>) -> Result<Self, String> {
        let edgion_resources::resources::edgion_plugins::EdgionPlugin::ForwardAuth(config) = input.plugin() else {
            return Err("owning ForwardAuth configuration required".into());
        };
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        Ok(Self::from_client(config, input.build_client()?))
    }

    fn from_client(config: &ForwardAuthConfig, client: ManagedOutboundClient) -> Self {
        ForwardAuth {
            client: Arc::new(client),
            status_on_error: config.conn.status_on_error,
            decision: config.decision.clone(),
            forward_body: config.forward_body,
        }
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
        let Ok(mut resp) = ResponseHeader::build(status, None) else {
            return PluginRunningResult::ErrTerminateRequest;
        };

        // Received connection nominations belong to the auth exchange, including
        // those in repeated field lines. An opaque nominator makes copying any
        // received field unsafe; the bounded denial body can still be relayed.
        let mut nominated = Vec::new();
        let mut invalid_connection = false;
        if let Some((headers, _)) = forwarded_headers {
            for value in headers.get_all(http::header::CONNECTION) {
                match value.to_str() {
                    Ok(value) => nominated.extend(connection_listed_hop_by_hop(value)),
                    Err(_) => invalid_connection = true,
                }
            }
        }
        if invalid_connection {
            plugin_log.push_err("invalid auth Connection header");
        }
        let can_copy = |name: &str| {
            !invalid_connection
                && !is_protected_response_header(name)
                && !nominated.iter().any(|listed| listed.eq_ignore_ascii_case(name))
        };
        if let Some(content_type) = content_type.filter(|_| can_copy("content-type")) {
            let _ = resp.insert_header("Content-Type", content_type);
        }
        let _ = resp.insert_header("Connection", "close");

        // Add configured client_headers from the auth service response in
        // config order. The first retained value replaces any existing field,
        // and later values remain separate response field lines.
        if let Some((auth_headers, client_headers)) = forwarded_headers {
            for (index, name) in client_headers.iter().enumerate() {
                if !can_copy(name) {
                    continue;
                }
                if client_headers[..index]
                    .iter()
                    .any(|prior| prior.eq_ignore_ascii_case(name))
                {
                    continue;
                }

                let mut inserted = false;
                for value in auth_headers
                    .get_all(name.as_str())
                    .iter()
                    .filter_map(|value| value.to_str().ok())
                {
                    // ResponseHeader's borrowed-name conversion is restricted
                    // to static strings; keep the configured name owned at the
                    // insertion boundary while borrowing the auth value.
                    let result = if inserted {
                        resp.append_header(name.clone(), value).map(|_| ())
                    } else {
                        resp.insert_header(name.clone(), value)
                    };
                    if result.is_err() {
                        plugin_log.push_err("invalid response header");
                    } else {
                        inserted = true;
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
        let Ok(resp) = result else {
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
        };
        self.handle_outbound_response(session, plugin_log, resp).await
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/forward_auth/plugin_test.rs"]
mod tests;

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
//! 1. Build auth request with original request metadata
//! 2. Send to external auth service
//! 3. If auth service returns 2xx → copy upstream_headers, forward to upstream
//! 4. If auth service returns non-2xx → return auth service's status & body to client
//! 5. If auth service is unreachable → return 503
//!
//! ## Configuration Examples
//!
//! ### Basic: forward all headers
//! ```yaml
//! type: ForwardAuth
//! config:
//!   uri: "http://auth-service:8080/verify"
//!   upstreamHeaders:
//!     - X-User-ID
//!     - X-User-Role
//! ```
//!
//! ### Selective: forward specific headers only
//! ```yaml
//! type: ForwardAuth
//! config:
//!   uri: "https://auth.example.com/verify"
//!   requestMethod: POST
//!   timeoutMs: 5000
//!   requestHeaders:
//!     - Authorization
//!     - Cookie
//!   upstreamHeaders:
//!     - X-User-ID
//!   clientHeaders:
//!     - WWW-Authenticate
//!   successStatusCodes: [200, 204]
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use pingora_http::ResponseHeader;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;

const MAX_ERROR_BODY_BYTES: usize = 4096;

use crate::plugins::http::common::auth_common::apply_auth_failure_delay;
use crate::plugins::http::common::http_client::{
    connection_listed_hop_by_hop, get_http_client, is_hop_by_hop_or_listed,
};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::ForwardAuthConfig;

pub struct ForwardAuth {
    name: String,
    config: ForwardAuthConfig,
}

impl ForwardAuth {
    pub fn new(config: &ForwardAuthConfig) -> Self {
        ForwardAuth {
            name: "ForwardAuth".to_string(),
            config: config.clone(),
        }
    }

    /// Build the header map to send to the auth service.
    ///
    /// Two modes:
    /// 1. request_headers is None/empty → forward ALL headers (skip hop-by-hop)
    /// 2. request_headers is specified → forward ONLY listed headers
    ///
    /// In both cases:
    /// - Hop-by-hop headers are excluded
    /// - Original Host is preserved via X-Forwarded-Host
    /// - Original URI and Method are passed via standard headers
    fn build_auth_headers(&self, session: &dyn PluginSession) -> HeaderMap {
        let mut headers = HeaderMap::new();

        match &self.config.request_headers {
            // Mode 1: Forward ALL headers (skip hop-by-hop)
            None => {
                self.forward_all_headers(session, &mut headers);
            }
            // Mode 2: Forward ONLY specified headers (or all if empty list)
            Some(allowed) => {
                if allowed.is_empty() {
                    // Empty list = same as None, forward all
                    self.forward_all_headers(session, &mut headers);
                } else {
                    for header_name in allowed {
                        if let Some(value) = session.header_value(header_name) {
                            if let (Ok(hn), Ok(hv)) = (
                                HeaderName::from_bytes(header_name.as_bytes()),
                                HeaderValue::from_str(&value),
                            ) {
                                headers.insert(hn, hv);
                            }
                        }
                    }
                }
            }
        }

        // Always set standard forwarding headers for auth service context
        let original_host = session.header_value("host").unwrap_or_default();
        let original_uri = session.get_path().to_string();
        let original_method = session.get_method().to_string();

        if let Ok(v) = HeaderValue::from_str(&original_host) {
            headers.insert("X-Forwarded-Host", v);
        }
        if let Ok(v) = HeaderValue::from_str(&original_uri) {
            headers.insert("X-Forwarded-Uri", v);
        }
        if let Ok(v) = HeaderValue::from_str(&original_method) {
            headers.insert("X-Forwarded-Method", v);
        }
        if let Some(query) = session.get_query() {
            if let Ok(v) = HeaderValue::from_str(&query) {
                headers.insert("X-Forwarded-Query", v);
            }
        }

        headers
    }

    /// Forward all request headers from session, skipping hop-by-hop headers
    /// (RFC 9110 §7.6.1 — both the canonical set and names dynamically listed
    /// in the request's `Connection:` header value).
    fn forward_all_headers(&self, session: &dyn PluginSession, headers: &mut HeaderMap) {
        let connection_value = session.header_value("connection").unwrap_or_default();
        let dynamic_listed = connection_listed_hop_by_hop(&connection_value);
        for (name, value) in session.request_headers() {
            if !is_hop_by_hop_or_listed(&name, &dynamic_listed) {
                if let (Ok(hn), Ok(hv)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(&value)) {
                    headers.insert(hn, hv);
                }
            }
        }
    }

    /// Write an error response directly to the client and return ErrTerminateRequest.
    async fn send_error_response(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        status: u16,
        body: &str,
        extra_headers: Option<&Vec<(String, String)>>,
    ) -> PluginRunningResult {
        let mut resp = match ResponseHeader::build(status, None) {
            Ok(r) => r,
            Err(_) => return PluginRunningResult::ErrTerminateRequest,
        };

        let _ = resp.insert_header("Content-Type", "application/json");
        let _ = resp.insert_header("Connection", "close");

        // Add extra headers (e.g., client_headers from auth service response).
        // The values were already validated by HeaderValue::to_str() upstream,
        // so insert_header failures here indicate an invalid header name
        // from the operator's `client_headers` config (non-token bytes, etc.).
        if let Some(headers) = extra_headers {
            for (name, value) in headers.iter() {
                if resp.insert_header(name.clone(), value.clone()).is_err() {
                    plugin_log.push_err("invalid response header");
                }
            }
        }

        let _ = session.write_response_header(Box::new(resp), false).await;
        let _ = session
            .write_response_body(Some(Bytes::from(body.to_string())), true)
            .await;
        session.shutdown().await;

        PluginRunningResult::ErrTerminateRequest
    }

    /// Check whether the auth response status code indicates success.
    fn is_success_status(&self, status: u16) -> bool {
        match &self.config.success_status_codes {
            Some(codes) => codes.contains(&status),
            None => (200..300).contains(&status),
        }
    }
}

#[async_trait]
impl RequestFilter for ForwardAuth {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let client = get_http_client();

        // Build auth request
        let auth_headers = self.build_auth_headers(session);
        let method: reqwest::Method = self.config.request_method.parse().unwrap_or(reqwest::Method::GET);
        let timeout = Duration::from_millis(self.config.timeout_ms);

        // Send auth request (no body)
        let resp = match client
            .request(method, &self.config.uri)
            .headers(auth_headers)
            .timeout(timeout)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(_) => {
                // Auth service unreachable (connection refused / DNS fail / timeout).
                // Surface in access log `errors` for gateway-level visibility; PluginLog
                // short tag captures the per-request plugin-side detail.
                session.add_error(edgion_resources::EdgionStatus::ForwardAuthServiceUnreachable);
                if self.config.allow_degradation {
                    plugin_log.push_err("upstream unreachable:degraded");
                    return PluginRunningResult::GoodNext;
                }

                plugin_log.push_err("upstream unreachable");
                return self
                    .send_error_response(
                        session,
                        plugin_log,
                        self.config.status_on_error,
                        r#"{"message":"Auth service unavailable"}"#,
                        None,
                    )
                    .await;
            }
        };

        let status = resp.status().as_u16();

        if self.is_success_status(status) {
            // Auth passed: copy specified response headers to upstream request
            let resp_headers = resp.headers().clone();
            for header_name in &self.config.upstream_headers {
                if let Some(value) = resp_headers.get(header_name.as_str()) {
                    if let Ok(v) = value.to_str() {
                        let _ = session.set_request_header(header_name, v);
                    }
                }
            }
            // Hide credentials if configured
            if self.config.hide_credentials {
                let _ = session.remove_request_header("authorization");
            }
            plugin_log.push_fmt("auth pass {}", la![status.to_string().as_str()]);
            PluginRunningResult::GoodNext
        } else {
            // Auth failed: collect client_headers from auth response to add to error response
            let mut extra_headers: Vec<(String, String)> = Vec::new();
            for header_name in &self.config.client_headers {
                if let Some(value) = resp.headers().get(header_name.as_str()) {
                    if let Ok(v) = value.to_str() {
                        extra_headers.push((header_name.clone(), v.to_string()));
                    }
                }
            }

            // Read auth service's error body with a size limit to prevent OOM
            // from a misbehaving auth service returning an unbounded response.
            let body = {
                let mut buf = Vec::with_capacity(MAX_ERROR_BODY_BYTES);
                let mut stream = resp.bytes_stream();
                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(buf.len());
                            if remaining == 0 {
                                break;
                            }
                            buf.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                        }
                        Err(_) => break,
                    }
                }
                String::from_utf8_lossy(&buf).into_owned()
            };

            plugin_log.push_fmt("auth deny {}", la![status.to_string().as_str()]);
            apply_auth_failure_delay(self.config.auth_failure_delay_ms).await;
            self.send_error_response(session, plugin_log, status, &body, Some(&extra_headers))
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    #[test]
    fn test_is_success_status_custom() {
        let config = ForwardAuthConfig {
            uri: "http://auth-service/verify".to_string(),
            success_status_codes: Some(vec![200, 204]),
            ..Default::default()
        };
        let plugin = ForwardAuth::new(&config);

        assert!(plugin.is_success_status(200));
        assert!(plugin.is_success_status(204));
        assert!(!plugin.is_success_status(201)); // Not in custom list
        assert!(!plugin.is_success_status(401));
    }

    #[test]
    fn test_build_auth_headers_selective() {
        let config = ForwardAuthConfig {
            uri: "http://auth-service/verify".to_string(),
            request_headers: Some(vec!["Authorization".to_string(), "Cookie".to_string()]),
            ..Default::default()
        };
        let plugin = ForwardAuth::new(&config);

        let mut mock_session = MockPluginSession::new();

        mock_session
            .expect_header_value()
            .withf(|name| name == "Authorization")
            .return_const(Some("Bearer token123".to_string()));
        mock_session
            .expect_header_value()
            .withf(|name| name == "Cookie")
            .return_const(Some("session=abc".to_string()));
        mock_session
            .expect_header_value()
            .withf(|name| name == "host")
            .return_const(Some("example.com".to_string()));
        mock_session.expect_get_path().return_const("/api/data".to_string());
        mock_session.expect_get_method().return_const("GET".to_string());
        mock_session.expect_get_query().return_const(None);

        let headers = plugin.build_auth_headers(&mock_session);

        // Check that only requested headers + X-Forwarded-* are present
        assert_eq!(
            headers.get("Authorization").unwrap().to_str().unwrap(),
            "Bearer token123"
        );
        assert_eq!(headers.get("Cookie").unwrap().to_str().unwrap(), "session=abc");
        assert_eq!(
            headers.get("X-Forwarded-Host").unwrap().to_str().unwrap(),
            "example.com"
        );
        assert_eq!(headers.get("X-Forwarded-Uri").unwrap().to_str().unwrap(), "/api/data");
        assert_eq!(headers.get("X-Forwarded-Method").unwrap().to_str().unwrap(), "GET");
    }
}

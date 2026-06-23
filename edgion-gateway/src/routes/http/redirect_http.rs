//! HTTP to HTTPS Redirect Implementation
//!
//! A simple ProxyHttp implementation that redirects all HTTP requests to HTTPS.
//! This is enabled via Gateway annotation `edgion.io/http-to-https-redirect: "true"`.

use async_trait::async_trait;
use http::StatusCode;
use pingora_core::prelude::HttpPeer;
use pingora_http::ResponseHeader;
use pingora_proxy::{ProxyHttp, Session};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

use crate::ctx::NormalizedHostname;
use crate::observe::AccessLogger;
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfig;

#[derive(Serialize)]
struct RedirectRequestInfo<'a> {
    #[serde(rename = "client-addr")]
    client_addr: &'a str,
    host: &'a str,
    path: &'a str,
    status: u16,
}

#[derive(Serialize)]
struct RedirectLogEntry<'a> {
    ts: i64,
    #[serde(rename = "type")]
    log_type: &'static str,
    request_info: RedirectRequestInfo<'a>,
    redirect_url: &'a str,
    latency_ms: u64,
    error: Option<String>,
}

/// Context for HTTP redirect - tracks request metadata for access logging
pub struct RedirectContext {
    pub start_time: Instant,
    pub client_addr: String,
    pub host: String,
    pub path: String,
    pub status: u16,
    pub redirect_url: String,
}

/// HTTP to HTTPS redirect proxy
///
/// This is a minimal ProxyHttp implementation that returns a 301 redirect
/// for all requests, pointing to the HTTPS version of the URL.
///
/// # Example
///
/// Request: `GET http://example.com/path?query=1`
/// Response: `301 Moved Permanently` with `Location: https://example.com/path?query=1`
pub struct EdgionHttpRedirectProxy {
    /// Target HTTPS port (default: 443)
    pub https_port: u16,
    /// Access logger for recording redirect events
    pub access_logger: Arc<AccessLogger>,
    /// Carries `SecurityProtectConfig.reject_duplicate_host` so the redirect
    /// listener applies the same RFC 9110 §7.2 host smuggling defense as the
    /// main proxy path.
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
}

impl EdgionHttpRedirectProxy {
    /// Create a new HTTP to HTTPS redirect handler
    pub fn new(
        https_port: u16,
        access_logger: Arc<AccessLogger>,
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
    ) -> Self {
        Self {
            https_port,
            access_logger,
            edgion_gateway_config,
        }
    }

    /// Whether the request authority is absent for redirect purposes.
    ///
    /// Returns `true` when the raw host is missing, whitespace-only, or collapses
    /// to empty after port stripping (e.g. a bare `:8080`). Uses the same
    /// normalization as the main proxy path so the redirect listener and the
    /// proxy agree on what counts as a valid Host (RFC 9112 §3.2 / Envoy parity).
    #[inline]
    fn host_is_missing(raw_host: &str) -> bool {
        NormalizedHostname::new(raw_host).is_empty()
    }

    /// Build the HTTPS redirect URL from host and URI components
    ///
    /// # Arguments
    /// * `host` - The Host header value (may include port)
    /// * `uri` - The request URI path and query string
    #[inline]
    fn build_redirect_url_from_parts(&self, host: &str, uri: &str) -> String {
        // Remove port from host if present (we'll add the HTTPS port).
        // IPv6-aware: `[::1]:8443` → `[::1]` (brackets preserved for the redirect URL).
        let host_without_port = crate::util::net::host_without_port(host);

        // Build redirect URL
        if self.https_port == 443 {
            format!("https://{}{}", host_without_port, uri)
        } else {
            format!("https://{}:{}{}", host_without_port, self.https_port, uri)
        }
    }
}

#[async_trait]
impl ProxyHttp for EdgionHttpRedirectProxy {
    type CTX = RedirectContext;

    fn new_ctx(&self) -> Self::CTX {
        RedirectContext {
            start_time: Instant::now(),
            client_addr: String::new(),
            host: String::new(),
            path: String::new(),
            status: 0,
            redirect_url: String::new(),
        }
    }

    /// This should never be called since we always return early in request_filter
    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        Err(pingora_core::Error::new(pingora_core::ErrorType::InternalError)
            .more_context("Redirect handler should not reach upstream_peer"))
    }

    /// Intercept all requests and return a 301 redirect
    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        ctx.client_addr = session.client_addr().map(|a| a.to_string()).unwrap_or_default();

        let req_header = session.req_header();

        // RFC 9110 §7.2 — duplicate Host header rejection. Mirrors the data-plane
        // proxy gate in `pg_request_filter::build_request_metadata` so attackers
        // cannot smuggle a Host through the HTTP→HTTPS redirect listener (which
        // would otherwise echo the attacker-controlled first Host into the 301
        // `Location:` URL).
        let reject_duplicate_host = self
            .edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .map(|s| s.reject_duplicate_host)
            .unwrap_or(true);
        if reject_duplicate_host && req_header.headers.get_all("host").iter().count() > 1 {
            ctx.status = 400;
            let mut resp = ResponseHeader::build(StatusCode::BAD_REQUEST, Some(2))?;
            resp.insert_header("Content-Length", "0")?;
            resp.insert_header("Connection", "close")?;
            session.write_response_header(Box::new(resp), true).await?;
            return Ok(true);
        }

        // RFC 9112 §3.2 — an HTTP/1.1 request whose Host is missing, invalid, or
        // collapses to empty (e.g. whitespace-only or a bare `:port`) must be
        // rejected with 400, not silently rewritten. This mirrors the main proxy
        // `HostMissing` guard in `pg_request_filter::build_request_metadata`, and
        // matches Envoy, which 400s requests lacking a valid authority. We must
        // NOT fall back to `localhost`, which would emit an attacker-unrelated
        // authority into the 301 `Location:`.
        let raw_host = req_header
            .uri
            .host()
            .or_else(|| req_header.headers.get("host").and_then(|h| h.to_str().ok()))
            .unwrap_or("");
        if Self::host_is_missing(raw_host) {
            ctx.status = 400;
            let mut resp = ResponseHeader::build(StatusCode::BAD_REQUEST, Some(2))?;
            resp.insert_header("Content-Length", "0")?;
            resp.insert_header("Connection", "close")?;
            session.write_response_header(Box::new(resp), true).await?;
            return Ok(true);
        }

        ctx.host = raw_host.to_string();
        ctx.path = req_header
            .uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());

        let redirect_url = self.build_redirect_url_from_parts(&ctx.host, &ctx.path);
        ctx.status = 301;

        ctx.redirect_url = redirect_url.clone();

        let mut resp = ResponseHeader::build(StatusCode::MOVED_PERMANENTLY, Some(3))?;
        resp.insert_header("Location", &redirect_url)?;
        resp.insert_header("Content-Length", "0")?;
        resp.insert_header("Connection", "close")?;

        // Send response and signal early return (true = don't continue to upstream)
        session.write_response_header(Box::new(resp), true).await?;

        Ok(true)
    }

    /// Log the redirect request after completion
    async fn logging(&self, _session: &mut Session, e: Option<&pingora_core::Error>, ctx: &mut Self::CTX)
    where
        Self::CTX: Send + Sync,
    {
        let latency_ms = Instant::now().saturating_duration_since(ctx.start_time).as_millis() as u64;
        let error_msg = e.map(|err| err.to_string());

        let entry = RedirectLogEntry {
            ts: chrono::Utc::now().timestamp_millis(),
            log_type: "redirect",
            request_info: RedirectRequestInfo {
                client_addr: &ctx.client_addr,
                host: &ctx.host,
                path: &ctx.path,
                status: ctx.status,
            },
            redirect_url: &ctx.redirect_url,
            latency_ms,
            error: error_msg,
        };

        if let Ok(s) = serde_json::to_string(&entry) {
            self.access_logger.send(s).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigSpec;

    fn test_access_logger() -> Arc<AccessLogger> {
        Arc::new(AccessLogger::new())
    }

    fn test_gateway_config() -> Arc<EdgionGatewayConfig> {
        Arc::new(EdgionGatewayConfig::new("test", EdgionGatewayConfigSpec::default()))
    }

    #[test]
    fn test_redirect_creation() {
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger(), test_gateway_config());
        assert_eq!(redirect.https_port, 443);

        let redirect = EdgionHttpRedirectProxy::new(8443, test_access_logger(), test_gateway_config());
        assert_eq!(redirect.https_port, 8443);
    }

    #[test]
    fn test_build_redirect_url_default_port() {
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger(), test_gateway_config());

        // Simple path
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com", "/"),
            "https://example.com/"
        );

        // Path with query string
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com", "/api/users?page=1&limit=10"),
            "https://example.com/api/users?page=1&limit=10"
        );

        // Host with port should strip the port
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com:80", "/path"),
            "https://example.com/path"
        );
    }

    #[test]
    fn test_build_redirect_url_custom_port() {
        let redirect = EdgionHttpRedirectProxy::new(8443, test_access_logger(), test_gateway_config());

        // Simple path
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com", "/"),
            "https://example.com:8443/"
        );

        // Path with query string
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com", "/api/users?page=1"),
            "https://example.com:8443/api/users?page=1"
        );

        // Host with port should be replaced with custom port
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com:8080", "/path"),
            "https://example.com:8443/path"
        );
    }

    #[test]
    fn test_build_redirect_url_special_cases() {
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger(), test_gateway_config());

        // IPv4 address
        assert_eq!(
            redirect.build_redirect_url_from_parts("192.168.1.1", "/"),
            "https://192.168.1.1/"
        );

        // IPv4 with port
        assert_eq!(
            redirect.build_redirect_url_from_parts("192.168.1.1:8080", "/api"),
            "https://192.168.1.1/api"
        );

        // Subdomain
        assert_eq!(
            redirect.build_redirect_url_from_parts("api.example.com", "/v1/users"),
            "https://api.example.com/v1/users"
        );

        // Complex query string
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com", "/search?q=hello+world&lang=zh"),
            "https://example.com/search?q=hello+world&lang=zh"
        );

        // Path with special characters
        assert_eq!(
            redirect.build_redirect_url_from_parts("example.com", "/path/to/%C3%A9%C3%B1"),
            "https://example.com/path/to/%C3%A9%C3%B1"
        );
    }

    #[test]
    fn test_host_is_missing() {
        // Missing / empty / whitespace-only Host must be treated as absent (→ 400).
        assert!(EdgionHttpRedirectProxy::host_is_missing(""));
        assert!(EdgionHttpRedirectProxy::host_is_missing("   "));
        assert!(EdgionHttpRedirectProxy::host_is_missing("\t"));
        // A bare `:port` with no authority collapses to empty after port strip.
        assert!(EdgionHttpRedirectProxy::host_is_missing(":8080"));

        // Valid authorities are accepted (→ 301).
        assert!(!EdgionHttpRedirectProxy::host_is_missing("example.com"));
        assert!(!EdgionHttpRedirectProxy::host_is_missing("example.com:80"));
        assert!(!EdgionHttpRedirectProxy::host_is_missing("192.168.1.1"));
        // IPv6 literal (bracketed) with and without port.
        assert!(!EdgionHttpRedirectProxy::host_is_missing("[::1]"));
        assert!(!EdgionHttpRedirectProxy::host_is_missing("[2001:db8::1]:8443"));
    }

    #[test]
    fn test_new_ctx() {
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger(), test_gateway_config());
        let _ctx = redirect.new_ctx(); // Should not panic
    }
}

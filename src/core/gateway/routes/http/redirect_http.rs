//! HTTP to HTTPS Redirect Implementation
//!
//! A simple ProxyHttp implementation that redirects all HTTP requests to HTTPS.
//! This is enabled via Gateway annotation `edgion.io/http-to-https-redirect: "true"`.

use async_trait::async_trait;
use http::StatusCode;
use pingora_core::prelude::HttpPeer;
use pingora_http::ResponseHeader;
use pingora_proxy::{ProxyHttp, Session};
use std::sync::Arc;
use std::time::Instant;

use crate::core::gateway::observe::AccessLogger;

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
}

impl EdgionHttpRedirectProxy {
    /// Create a new HTTP to HTTPS redirect handler
    pub fn new(https_port: u16, access_logger: Arc<AccessLogger>) -> Self {
        Self {
            https_port,
            access_logger,
        }
    }

    /// Build the HTTPS redirect URL from host and URI components
    ///
    /// # Arguments
    /// * `host` - The Host header value (may include port)
    /// * `uri` - The request URI path and query string
    #[inline]
    fn build_redirect_url_from_parts(&self, host: &str, uri: &str) -> String {
        // Remove port from host if present (we'll add the HTTPS port)
        let host_without_port = host.split(':').next().unwrap_or(host);

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
        // This should never be reached as request_filter always returns true (early response)
        Err(pingora_core::Error::new(pingora_core::ErrorType::InternalError)
            .more_context("Redirect handler should not reach upstream_peer"))
    }

    /// Intercept all requests and return a 301 redirect
    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        // Populate context for access logging
        ctx.client_addr = session.client_addr().map(|a| a.to_string()).unwrap_or_default();

        let req_header = session.req_header();
        ctx.host = req_header
            .headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost")
            .to_string();
        ctx.path = req_header
            .uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());

        let redirect_url = self.build_redirect_url_from_parts(&ctx.host, &ctx.path);
        ctx.status = 301;

        ctx.redirect_url = redirect_url.clone();

        // Build 301 response
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
        let latency_ms = ctx.start_time.elapsed().as_millis() as u64;
        let error_msg = e.map(|err| err.to_string());

        let log_entry = serde_json::json!({
            "ts": chrono::Utc::now().timestamp_millis(),
            "type": "redirect",
            "request_info": {
                "client-addr": ctx.client_addr,
                "host": ctx.host,
                "path": ctx.path,
                "status": ctx.status,
            },
            "redirect_url": ctx.redirect_url,
            "latency_ms": latency_ms,
            "error": error_msg,
        });

        if tracing::level_filters::LevelFilter::current() >= tracing::level_filters::LevelFilter::DEBUG {
            tracing::debug!(
                access_log = %log_entry,
                "Redirect access log"
            );
        }

        self.access_logger.send(log_entry.to_string()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_access_logger() -> Arc<AccessLogger> {
        Arc::new(AccessLogger::new())
    }

    #[test]
    fn test_redirect_creation() {
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger());
        assert_eq!(redirect.https_port, 443);

        let redirect = EdgionHttpRedirectProxy::new(8443, test_access_logger());
        assert_eq!(redirect.https_port, 8443);
    }

    #[test]
    fn test_build_redirect_url_default_port() {
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger());

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
        let redirect = EdgionHttpRedirectProxy::new(8443, test_access_logger());

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
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger());

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
            redirect.build_redirect_url_from_parts("example.com", "/path/to/%E4%B8%AD%E6%96%87"),
            "https://example.com/path/to/%E4%B8%AD%E6%96%87"
        );
    }

    #[test]
    fn test_new_ctx() {
        let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger());
        let _ctx = redirect.new_ctx(); // Should not panic
    }
}

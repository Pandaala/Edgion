//! HTTP to HTTPS Redirect Implementation
//!
//! A simple ProxyHttp implementation that redirects all HTTP requests to HTTPS.
//! This is enabled via Gateway annotation `edgion.io/http-to-https-redirect: "true"`.

use async_trait::async_trait;
use http::StatusCode;
use pingora_core::prelude::HttpPeer;
use pingora_http::ResponseHeader;
use pingora_proxy::{ProxyHttp, Session};

/// Simple context for HTTP redirect - minimal state needed
pub struct RedirectContext;

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
}

impl EdgionHttpRedirectProxy {
    /// Create a new HTTP to HTTPS redirect handler
    pub fn new(https_port: u16) -> Self {
        Self { https_port }
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

    /// Build the HTTPS redirect URL from a Session
    fn build_redirect_url(&self, session: &Session) -> String {
        let req_header = session.req_header();

        // Get host from Host header
        let host = req_header
            .headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost");

        // Get the original URI (path + query)
        let uri = req_header.uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

        self.build_redirect_url_from_parts(host, uri)
    }
}

#[async_trait]
impl ProxyHttp for EdgionHttpRedirectProxy {
    type CTX = RedirectContext;

    fn new_ctx(&self) -> Self::CTX {
        RedirectContext
    }

    /// This should never be called since we always return early in request_filter
    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        // This should never be reached as request_filter always returns true (early response)
        Err(pingora_core::Error::new(pingora_core::ErrorType::InternalError)
            .more_context("Redirect handler should not reach upstream_peer"))
    }

    /// Intercept all requests and return a 301 redirect
    async fn request_filter(&self, session: &mut Session, _ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        let redirect_url = self.build_redirect_url(session);

        tracing::debug!(
            component = "http_redirect",
            redirect_url = %redirect_url,
            "Redirecting HTTP to HTTPS"
        );

        // Build 301 response
        let mut resp = ResponseHeader::build(StatusCode::MOVED_PERMANENTLY, Some(3))?;
        resp.insert_header("Location", &redirect_url)?;
        resp.insert_header("Content-Length", "0")?;
        resp.insert_header("Connection", "close")?;

        // Send response and signal early return (true = don't continue to upstream)
        session.write_response_header(Box::new(resp), true).await?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redirect_creation() {
        let redirect = EdgionHttpRedirectProxy::new(443);
        assert_eq!(redirect.https_port, 443);

        let redirect = EdgionHttpRedirectProxy::new(8443);
        assert_eq!(redirect.https_port, 8443);
    }

    #[test]
    fn test_build_redirect_url_default_port() {
        let redirect = EdgionHttpRedirectProxy::new(443);

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
        let redirect = EdgionHttpRedirectProxy::new(8443);

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
        let redirect = EdgionHttpRedirectProxy::new(443);

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
        let redirect = EdgionHttpRedirectProxy::new(443);
        let _ctx = redirect.new_ctx(); // Should not panic
    }
}

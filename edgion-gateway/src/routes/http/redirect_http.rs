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
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    direct_peer_ip: &'a str,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    remote_ip: &'a str,
    host: &'a str,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    path: &'a str,
    status: u16,
}

#[derive(Serialize)]
struct RedirectLogEntry<'a> {
    ts: i64,
    #[serde(rename = "type")]
    log_type: &'static str,
    request_info: RedirectRequestInfo<'a>,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    redirect_url: &'a str,
    latency_ms: u64,
    #[serde(serialize_with = "edgion_resources::sensitive::masked_opt::serialize")]
    error: Option<String>,
}

/// Context for HTTP redirect - tracks request metadata for access logging
pub struct RedirectContext {
    pub start_time: Instant,
    pub direct_peer_ip: String,
    pub remote_ip: String,
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
            direct_peer_ip: String::new(),
            remote_ip: String::new(),
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
    ///
    /// Redirects and Host rejections retain Pingora's normal connection policy.
    /// The session owns keepalive and generates the matching Connection header.
    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        let raw = session
            .as_downstream()
            .digest()
            .and_then(|d| d.socket_digest.as_ref())
            .and_then(|d| d.raw_peer_addr());
        let addrs = crate::util::resolve_transport_client_addrs(session.client_addr(), raw, "");
        ctx.remote_ip = addrs.remote_ip;
        ctx.direct_peer_ip = addrs.direct_peer_ip;

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
            session.write_response_header(Box::new(resp), true).await?;
            return Ok(true);
        }

        ctx.host = raw_host.to_string();
        let path_and_query = req_header.uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
        ctx.path = match req_header.uri.path() {
            "" => "/".to_string(),
            path => path.to_string(),
        };

        let redirect_url = self.build_redirect_url_from_parts(&ctx.host, path_and_query);
        ctx.status = 301;

        ctx.redirect_url = self.build_redirect_url_from_parts(&ctx.host, &ctx.path);

        let mut resp = ResponseHeader::build(StatusCode::MOVED_PERMANENTLY, Some(3))?;
        resp.insert_header("Location", &redirect_url)?;
        resp.insert_header("Content-Length", "0")?;

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
                direct_peer_ip: &ctx.direct_peer_ip,
                remote_ip: &ctx.remote_ip,
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
    use edgion_resources::sensitive::{SensitiveMode, SENSITIVE_MODE};
    use std::sync::atomic::Ordering;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn assert_sensitive_log_in_subprocess(test_name: &str, assertion: impl FnOnce()) {
        const CHILD_ENV: &str = "EDGION_REDIRECT_SENSITIVE_LOG_CHILD";
        const TEST_MODULE: &str = "routes::http::redirect_http::tests";
        crate::test_subprocess::assert_in_subprocess(TEST_MODULE, test_name, CHILD_ENV, test_name, assertion);
    }

    fn redirect_log_json(error: Option<String>) -> serde_json::Value {
        serde_json::to_value(RedirectLogEntry {
            ts: 0,
            log_type: "redirect",
            request_info: RedirectRequestInfo {
                direct_peer_ip: "192.0.2.1",
                remote_ip: "198.51.100.2",
                host: "example.com",
                path: "/account/reset",
                status: 301,
            },
            redirect_url: "https://example.com/account/reset",
            latency_ms: 1,
            error,
        })
        .unwrap()
    }

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

    #[test]
    fn redirect_log_sensitive_fields_follow_the_shared_masking_contract() {
        assert_sensitive_log_in_subprocess(
            "redirect_log_sensitive_fields_follow_the_shared_masking_contract",
            || {
                for (mode, expected_path, expected_error) in [
                    (SensitiveMode::Replace, "***", "***"),
                    (SensitiveMode::Wrap, "<E>/account/reset</E>", "<E>private failure</E>"),
                    (SensitiveMode::Off, "/account/reset", "private failure"),
                ] {
                    SENSITIVE_MODE.store(mode as u8, Ordering::SeqCst);
                    let value = redirect_log_json(Some("private failure".to_string()));
                    assert_eq!(value["request_info"]["path"], expected_path);
                    assert_eq!(
                        value["redirect_url"],
                        match mode {
                            SensitiveMode::Replace => "***",
                            SensitiveMode::Wrap => "<E>https://example.com/account/reset</E>",
                            SensitiveMode::Off => "https://example.com/account/reset",
                        }
                    );
                    assert_eq!(value["error"], expected_error);
                }
            },
        );
    }

    #[test]
    fn absent_redirect_error_stays_null_under_masking() {
        assert_sensitive_log_in_subprocess("absent_redirect_error_stays_null_under_masking", || {
            SENSITIVE_MODE.store(SensitiveMode::Replace as u8, Ordering::SeqCst);
            assert!(redirect_log_json(None)["error"].is_null());
        });
    }

    #[tokio::test]
    async fn redirect_connection_policy_matches_wire_and_stream_reuse() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for (host_headers, expected_status) in [
                ("Host: example.com\r\n", 301),
                ("Host: example.com\r\nHost: other.example.com\r\n", 400),
                ("Host: :8080\r\n", 400),
            ] {
                for client_close in [false, true] {
                    let redirect = EdgionHttpRedirectProxy::new(443, test_access_logger(), test_gateway_config());
                    let (mut client, server) = tokio::io::duplex(4096);
                    let connection = if client_close { "Connection: close\r\n" } else { "" };
                    client
                        .write_all(format!("GET /first HTTP/1.1\r\n{host_headers}{connection}\r\n").as_bytes())
                        .await
                        .unwrap();
                    let mut session = Session::new_h1(Box::new(server));
                    assert!(session.read_request().await.unwrap());
                    let mut ctx = redirect.new_ctx();
                    assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());
                    assert_eq!(ctx.status, expected_status);

                    let reusable = session.downstream_session.finish().await.unwrap();
                    assert_eq!(reusable.is_some(), !client_close);
                    let mut wire = Vec::new();
                    while !wire.ends_with(b"\r\n\r\n") {
                        wire.push(client.read_u8().await.unwrap());
                    }
                    let wire = String::from_utf8(wire).unwrap();
                    assert!(wire.starts_with(&format!("HTTP/1.1 {expected_status}")));
                    let expected_connection = if client_close { "close" } else { "keep-alive" };
                    assert!(wire
                        .to_ascii_lowercase()
                        .contains(&format!("\r\nconnection: {expected_connection}\r\n")));
                    assert!(wire.to_ascii_lowercase().contains("\r\ncontent-length: 0\r\n"));

                    if let Some(reusable) = reusable {
                        // Send only after finishing the first request: this tests reuse,
                        // without relying on pipelining or dropping the live session.
                        let (stream, prefix) = reusable.into_parts();
                        assert!(prefix.is_none());
                        client
                            .write_all(b"GET /second HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
                            .await
                            .unwrap();
                        let mut session = Session::new_h1(stream);
                        assert!(session.read_request().await.unwrap());
                        let mut ctx = redirect.new_ctx();
                        assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());
                        assert!(session.downstream_session.finish().await.unwrap().is_none());
                        let mut second_wire = String::new();
                        client.read_to_string(&mut second_wire).await.unwrap();
                        assert!(second_wire.starts_with("HTTP/1.1 301"));
                        assert!(second_wire.contains("https://example.com/second"));
                        assert!(second_wire.to_ascii_lowercase().contains("\r\nconnection: close\r\n"));
                    } else {
                        assert_eq!(client.read(&mut [0; 1]).await.unwrap(), 0);
                    }
                }
            }
        })
        .await
        .expect("redirect connection-policy exchanges must finish without waiting for peer EOF");
    }

    #[tokio::test]
    async fn redirect_wire_preserves_query_percent_encoding_ipv6_port_and_client_requested_close() {
        let redirect = EdgionHttpRedirectProxy::new(8443, test_access_logger(), test_gateway_config());
        let (mut client, server) = tokio::io::duplex(4096);
        // Pingora derives the response Connection header from session keepalive;
        // exercise the client-requested close branch without changing that policy.
        client
            .write_all(
                b"GET /account%20reset?token=secret HTTP/1.1\r\nHost: [2001:db8::1]:8080\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        assert!(session.read_request().await.unwrap());
        let mut ctx = redirect.new_ctx();

        assert!(redirect.request_filter(&mut session, &mut ctx).await.unwrap());
        assert_eq!(ctx.path, "/account%20reset");
        assert_eq!(ctx.redirect_url, "https://[2001:db8::1]:8443/account%20reset");
        drop(session);

        let mut wire = Vec::new();
        client.read_to_end(&mut wire).await.unwrap();
        let wire = String::from_utf8(wire).unwrap();
        assert!(wire.starts_with("HTTP/1.1 301"));
        let header = |name: &str| {
            wire.split("\r\n")
                .filter_map(|line| line.split_once(": "))
                .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
        };
        assert_eq!(
            header("Location"),
            Some("https://[2001:db8::1]:8443/account%20reset?token=secret")
        );
        assert_eq!(header("Content-Length"), Some("0"));
        assert_eq!(header("Connection"), Some("close"));
        assert!(wire.ends_with("\r\n\r\n"));
        assert!(!ctx.path.contains("token="));
        assert!(!ctx.redirect_url.contains("token="));
    }
}

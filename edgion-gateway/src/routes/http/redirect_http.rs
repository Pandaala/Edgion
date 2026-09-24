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
    egc: Arc<crate::config::ResolvedGatewayConfig>,
    downstream_keepalive_initialized: bool,
    pub start_time: Instant,
    pub direct_peer_ip: String,
    pub remote_ip: String,
    pub host: String,
    pub path: String,
    pub status: u16,
    pub redirect_url: String,
    pub reject_duplicate_host: bool,
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
    /// Stable EGC binding shared with the listener configuration publisher.
    pub egc: Arc<crate::config::GatewayConfigHandle>,
}

impl EdgionHttpRedirectProxy {
    /// Create a new HTTP to HTTPS redirect handler
    pub fn new(
        https_port: u16,
        access_logger: Arc<AccessLogger>,
        egc: Arc<crate::config::GatewayConfigHandle>,
    ) -> Self {
        Self {
            https_port,
            access_logger,
            egc,
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
        let request_config = self.egc.load();
        RedirectContext {
            start_time: Instant::now(),
            direct_peer_ip: String::new(),
            remote_ip: String::new(),
            host: String::new(),
            path: String::new(),
            status: 0,
            redirect_url: String::new(),
            reject_duplicate_host: request_config.reject_duplicate_host,
            egc: request_config,
            downstream_keepalive_initialized: false,
        }
    }

    async fn early_request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<()> {
        super::proxy_http::apply_client_policy(session, &ctx.egc, &mut ctx.downstream_keepalive_initialized);
        Ok(())
    }

    fn persist_connection_context(
        &self,
        _session: &Session,
        ctx: &Self::CTX,
    ) -> Option<Box<dyn std::any::Any + Send + Sync>> {
        ctx.downstream_keepalive_initialized
            .then(|| Box::new(()) as Box<dyn std::any::Any + Send + Sync>)
    }

    fn on_connection_reuse(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
        _previous: Box<dyn std::any::Any + Send + Sync>,
    ) {
        ctx.downstream_keepalive_initialized = true;
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
        if ctx.reject_duplicate_host && req_header.headers.get_all("host").iter().count() > 1 {
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
#[path = "../../../tests/unit/routes/http/redirect_http_test.rs"]
mod tests;

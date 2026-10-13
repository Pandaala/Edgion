//! Real network [`HttpDnsFetcher`] built on the shared outbound reqwest client.
//!
//! The rendering/parsing/fallback core in [`super::httpdns`] is transport-free
//! and unit-tested against a fake fetcher; this module supplies the production
//! fetcher that actually issues the HTTPS request, honoring the resolver's
//! [`HttpDnsConnection`] (outbound TLS + request timeout) and enforcing an upper
//! bound on the response body so a malicious/buggy endpoint cannot stream an
//! unbounded body into memory.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use edgion_resources::resources::link_sys::httpdns::HttpDnsConnection;
use edgion_resources::LinkSys;

use super::httpdns::HttpDnsFetcher;
use super::{DnsError, ResolverSpec};
use crate::egress::{classify_ip, EgressPolicy, EgressResolver, IpClass};
use crate::link_sys_core::http_client::{build_outbound_client_with_tls_selection, OutboundClientConfig};
use crate::link_sys_core::outbound_tls::{GlobalOutboundTlsSnapshot, ValidatedLocalTlsSelection};
use crate::test_mode::is_integration_testing_mode;

/// Production [`HttpDnsFetcher`] over a `reqwest::Client`.
pub struct ReqwestHttpDnsFetcher {
    client: reqwest::Client,
    max_response_bytes: usize,
    allow_loopback: bool,
}

impl ReqwestHttpDnsFetcher {
    /// Build a fetcher from its actual HTTPDNS LinkSys owner, validating the
    /// entire configuration and owning TLS selection before construction.
    ///
    /// - Timeouts and native connection-pool bounds are resolved at preparation.
    /// - `tls` (when `enabled`) → outbound TLS policy (custom CA / mTLS / gated
    ///   `verify=false`). When absent, HTTPS uses the default system roots.
    /// - Proxies are disabled and redirects denied (SSRF hardening) by the
    ///   shared client factory's defaults.
    pub fn new(owner: Arc<LinkSys>) -> Result<Self, DnsError> {
        let candidate = super::HttpDnsCandidate::from_link_sys(owner)
            .ok_or_else(|| DnsError::Http("HTTP DNS input is not an HTTPDNS LinkSys".into()))?;
        candidate.build_fetcher()
    }

    pub(super) fn from_bound(
        conn: Option<&HttpDnsConnection>,
        proof: &ValidatedLocalTlsSelection<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
    ) -> Result<Self, DnsError> {
        if let Some(connection) = conn {
            if !connection.validate().is_empty() {
                return Err(DnsError::Http("invalid HTTP DNS connection settings".into()));
            }
        }
        let timeout_ms = conn
            .and_then(|c| c.timeout_ms)
            .unwrap_or(HttpDnsConnection::DEFAULT_TIMEOUT_MS);
        // Match the canonical resolver's test exception on the literal path too.
        let allow_loopback = conn.is_some_and(|c| c.allow_loopback) || cfg!(test) || is_integration_testing_mode();
        // Bootstrap is always System: resolving the endpoint through HTTP DNS
        // itself would recurse. Return only admitted addresses to the connector,
        // rather than checking DNS once and letting reqwest resolve it again.
        let resolver = EgressResolver::new_with_options(
            ResolverSpec::System,
            EgressPolicy { block_private: false },
            Duration::ZERO,
            allow_loopback,
            None,
        );
        let cfg = OutboundClientConfig {
            total_timeout: Some(Duration::from_millis(timeout_ms)),
            connect_timeout: Some(Duration::from_millis(
                conn.and_then(|c| c.connect_timeout_ms).unwrap_or(timeout_ms),
            )),
            pool_max_idle_per_host: Some(
                conn.and_then(|c| c.pool_max_idle_per_host)
                    .unwrap_or(HttpDnsConnection::DEFAULT_POOL_MAX_IDLE_PER_HOST),
            ),
            pool_idle_timeout: Some(Duration::from_millis(
                conn.and_then(|c| c.pool_idle_timeout_ms)
                    .unwrap_or(HttpDnsConnection::DEFAULT_POOL_IDLE_TIMEOUT_MS),
            )),
            disable_proxy: true,
            tls_context: Some("HttpDns"),
            dns_resolver: Some(Arc::new(resolver)),
            ..Default::default()
        };
        let client = build_outbound_client_with_tls_selection(cfg, proof, defaults)
            .map_err(|e| DnsError::Http(e.to_string()))?;
        Ok(Self {
            client,
            max_response_bytes: conn
                .and_then(|c| c.max_response_bytes)
                .unwrap_or(HttpDnsConnection::MAX_RESPONSE_BYTES),
            allow_loopback,
        })
    }
}

#[async_trait::async_trait]
impl HttpDnsFetcher for ReqwestHttpDnsFetcher {
    async fn get(&self, url: &str) -> Result<String, DnsError> {
        let url = reqwest::Url::parse(url).map_err(|_| DnsError::Http("invalid HTTP DNS endpoint URL".into()))?;
        // reqwest skips custom DNS for literals. Classify the normalized URL
        // host, including bracketed IPv6, and send this exact parsed URL.
        if let Some(ip) = url
            .host_str()
            .and_then(|host| super::strip_brackets(host).parse::<IpAddr>().ok())
        {
            match classify_ip(ip) {
                IpClass::Loopback if !self.allow_loopback => {
                    return Err(DnsError::Http("HTTP DNS endpoint blocked: loopback".into()));
                }
                IpClass::LinkLocal | IpClass::Unspecified => {
                    return Err(DnsError::Http(
                        "HTTP DNS endpoint blocked: link-local or unspecified".into(),
                    ));
                }
                _ => {}
            }
        }
        let mut resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| DnsError::Http(e.without_url().to_string()))?;
        if !resp.status().is_success() {
            return Err(DnsError::Http(format!(
                "http dns endpoint returned status {}",
                resp.status()
            )));
        }
        // Read the body in chunks and enforce `max_response_bytes` on the *actual*
        // bytes received (Content-Length may be absent or dishonest).
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| DnsError::Http(e.without_url().to_string()))?
        {
            if buf.len() + chunk.len() > self.max_response_bytes {
                return Err(DnsError::Http(format!(
                    "http dns response exceeds the {}-byte limit",
                    self.max_response_bytes
                )));
            }
            buf.extend_from_slice(&chunk);
        }
        String::from_utf8(buf).map_err(|e| DnsError::Parse(e.to_string()))
    }
}

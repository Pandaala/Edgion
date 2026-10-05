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

use super::httpdns::HttpDnsFetcher;
use super::{DnsError, ResolverSpec};
use crate::egress::{classify_ip, EgressPolicy, EgressResolver, IpClass};
use crate::link_sys_core::http_client::{build_outbound_client, OutboundClientConfig};
use crate::test_mode::is_integration_testing_mode;

/// Default per-request timeout when the resolver config does not set one.
const DEFAULT_HTTP_DNS_TIMEOUT_MS: u64 = 3000;

/// Upper bound on an HTTP DNS response body. A well-formed IP list is tiny; this
/// only guards against an endpoint that streams an unbounded body (the parser
/// separately caps the accepted IP count). 64 KiB is generous for any real
/// vendor response.
const MAX_HTTP_DNS_BODY_BYTES: usize = 64 * 1024;

/// Production [`HttpDnsFetcher`] over a `reqwest::Client`.
pub struct ReqwestHttpDnsFetcher {
    client: reqwest::Client,
    max_response_bytes: usize,
    allow_loopback: bool,
}

impl ReqwestHttpDnsFetcher {
    /// Build a fetcher from the resolver's optional connection settings.
    ///
    /// - `timeout_ms` → whole-request timeout (default [`DEFAULT_HTTP_DNS_TIMEOUT_MS`]).
    /// - `tls` (when `enabled`) → outbound TLS policy (custom CA / mTLS / gated
    ///   `verify=false`). When absent, HTTPS uses the default system roots.
    /// - Proxies are disabled and redirects denied (SSRF hardening) by the
    ///   shared client factory's defaults.
    pub fn new(conn: Option<&HttpDnsConnection>) -> Result<Self, DnsError> {
        let timeout_ms = conn.and_then(|c| c.timeout_ms).unwrap_or(DEFAULT_HTTP_DNS_TIMEOUT_MS);
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
        // Pass TLS only when explicitly enabled; the client factory treats
        // `None` as "no custom TLS" (default roots), per its contract.
        let tls = conn.and_then(|c| c.tls.clone()).filter(|t| t.enabled);
        let cfg = OutboundClientConfig {
            total_timeout: Some(Duration::from_millis(timeout_ms)),
            disable_proxy: true,
            tls,
            tls_context: Some("HttpDns"),
            dns_resolver: Some(Arc::new(resolver)),
            ..Default::default()
        };
        let client = build_outbound_client(cfg).map_err(|e| DnsError::Http(e.to_string()))?;
        Ok(Self {
            client,
            max_response_bytes: MAX_HTTP_DNS_BODY_BYTES,
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
            .map_err(|e| DnsError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(DnsError::Http(format!(
                "http dns endpoint returned status {}",
                resp.status()
            )));
        }
        // Read the body in chunks and enforce `max_response_bytes` on the *actual*
        // bytes received (Content-Length may be absent or dishonest).
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| DnsError::Http(e.to_string()))? {
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

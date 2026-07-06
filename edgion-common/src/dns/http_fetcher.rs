//! Real network [`HttpDnsFetcher`] built on the shared outbound reqwest client.
//!
//! The rendering/parsing/fallback core in [`super::httpdns`] is transport-free
//! and unit-tested against a fake fetcher; this module supplies the production
//! fetcher that actually issues the HTTPS request, honoring the resolver's
//! [`HttpDnsConnection`] (outbound TLS + request timeout) and enforcing an upper
//! bound on the response body so a malicious/buggy endpoint cannot stream an
//! unbounded body into memory.

use std::time::Duration;

use edgion_resources::resources::link_sys::httpdns::HttpDnsConnection;

use super::httpdns::HttpDnsFetcher;
use super::DnsError;
use crate::link_sys_core::http_client::{build_outbound_client, OutboundClientConfig};

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
        // Pass TLS only when explicitly enabled; the client factory treats
        // `None` as "no custom TLS" (default roots), per its contract.
        let tls = conn.and_then(|c| c.tls.clone()).filter(|t| t.enabled);
        let cfg = OutboundClientConfig {
            total_timeout: Some(Duration::from_millis(timeout_ms)),
            disable_proxy: true,
            tls,
            tls_context: Some("HttpDns"),
            ..Default::default()
        };
        let client = build_outbound_client(cfg).map_err(|e| DnsError::Http(e.to_string()))?;
        Ok(Self {
            client,
            max_response_bytes: MAX_HTTP_DNS_BODY_BYTES,
        })
    }

    /// Override the response-body cap. Test-only helper so a small oversized body
    /// can exercise the bound without allocating a 64 KiB fixture.
    #[cfg(test)]
    fn with_max_response_bytes(mut self, n: usize) -> Self {
        self.max_response_bytes = n;
        self
    }
}

#[async_trait::async_trait]
impl HttpDnsFetcher for ReqwestHttpDnsFetcher {
    async fn get(&self, url: &str) -> Result<String, DnsError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Spawn a one-shot HTTP/1.1 stub that replies with `body` and returns its URL.
    async fn spawn_stub(body: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut scratch = [0u8; 1024];
                let _ = sock.read(&mut scratch).await; // drain the request line/headers
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        format!("http://{addr}/d")
    }

    #[tokio::test]
    async fn fetches_body_from_stub() {
        let url = spawn_stub(r#"{"ips":["1.2.3.4"]}"#.to_string()).await;
        let fetcher = ReqwestHttpDnsFetcher::new(None).unwrap();
        let body = fetcher.get(&url).await.unwrap();
        assert!(body.contains("1.2.3.4"));
    }

    #[tokio::test]
    async fn rejects_body_over_limit() {
        let url = spawn_stub("x".repeat(100)).await;
        let fetcher = ReqwestHttpDnsFetcher::new(None).unwrap().with_max_response_bytes(8);
        let err = fetcher.get(&url).await.unwrap_err();
        assert!(
            matches!(err, DnsError::Http(_)),
            "expected size-limit error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn end_to_end_preset_parse_via_stub() {
        use edgion_resources::resources::link_sys::httpdns::{
            HttpDnsFallback, HttpDnsResolverConfig, HttpDnsResponseFormat,
        };

        // A generic (non-preset) config pointed at the stub, JSON `ips` shape.
        let url = spawn_stub(r#"{"ips":["93.184.216.34","93.184.216.35"]}"#.to_string()).await;
        let cfg = HttpDnsResolverConfig {
            preset: None,
            url_template: Some(format!("{url}?host={{domain}}")),
            response: HttpDnsResponseFormat {
                kind: edgion_resources::resources::link_sys::httpdns::HttpDnsBodyKind::Json,
                ip_path: Some("ips".to_string()),
                delimiter: None,
                ttl_path: None,
            },
            fallback: HttpDnsFallback::None,
            connection: None,
        };
        let fetcher = ReqwestHttpDnsFetcher::new(None).unwrap();
        let ips = super::super::httpdns::resolve_http_dns_with(&fetcher, &cfg, "api.example.com")
            .await
            .unwrap();
        assert_eq!(ips.len(), 2);
    }
}

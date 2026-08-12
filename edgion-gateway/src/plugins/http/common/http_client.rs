//! Shared HTTP client for plugins that need to call external services.
//!
//! Provides a global `reqwest::Client` singleton with connection pooling,
//! and utilities for hop-by-hop header filtering.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::plugins::http::common::http_client::get_http_client;
//!
//! let client = get_http_client();
//! let resp = client.get("http://auth-service/verify")
//!     .send()
//!     .await?;
//! ```
//!
//! ## Design Decisions
//!
//! - **Global singleton**: `reqwest::Client` maintains an internal connection pool.
//!   Sharing a single instance across all plugins maximizes connection reuse.
//! - **OnceLock lazy initialization**: Consistent with the project's pattern (e.g., RateLimit).
//!   The client is only created on first use.
//! - **No automatic redirects**: Auth-type requests should not follow redirects automatically.
//!   A 302 from an auth service typically indicates an authentication failure or redirect to login.
//! - **Default timeouts**: Prevent external services from hanging the gateway.
//!   Individual plugins can override per-request timeouts.

use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;

use crate::link_sys::http_client::{
    apply_outbound_policy, build_outbound_client, OutboundClientConfig, OutboundRedirect,
};

/// Global shared HTTP client with connection pooling.
///
/// Uses OnceLock for lazy initialization; reqwest::Client is designed to be shared.
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

/// Get an Arc-wrapped HTTP client for sharing (e.g. in spawned tasks).
/// Uses the default secure client.
pub fn get_http_client_arc() -> std::sync::Arc<Client> {
    static DEFAULT_ARC: OnceLock<std::sync::Arc<Client>> = OnceLock::new();
    DEFAULT_ARC
        .get_or_init(|| std::sync::Arc::new(get_http_client().clone()))
        .clone()
}

/// Get the global HTTP client instance.
///
/// The client is initialized once on first call with sensible defaults:
/// - Connection pool: up to 32 idle connections per host
/// - Request timeout: 10 seconds (adjustable per-request)
/// - Connect timeout: 5 seconds
/// - No automatic redirect following
/// - TLS enabled (rustls)
pub fn get_http_client() -> &'static Client {
    HTTP_CLIENT.get_or_init(|| {
        build_outbound_client(OutboundClientConfig {
            pool_max_idle_per_host: Some(32),
            connect_timeout: Some(Duration::from_secs(5)),
            total_timeout: Some(Duration::from_secs(10)),
            redirect: OutboundRedirect::Deny,
            // Plugin dependency traffic must never inherit HTTP(S)_PROXY from
            // the gateway process. Proxy routing would bypass the canonical
            // dial-time egress resolver/classifier.
            disable_proxy: true,
            ..Default::default()
        })
        .unwrap_or_else(|e| {
            tracing::error!(component = "http_client", error = %e, "Failed to build HTTP client");
            std::process::exit(1);
        })
    })
}

/// Build a dedicated outbound client for a webhook / forward_auth connection.
///
/// Returns `None` only when neither a TLS block nor a DNS resolver is supplied —
/// i.e. when the shared global client is safe to use. A URL target always supplies
/// a resolver, so it always gets a dedicated client (design spec 2.6).
pub fn build_webhook_client(
    tls_config: &Option<edgion_resources::resources::common::TlsConfig>,
    dns_resolver: Option<std::sync::Arc<dyn reqwest::dns::Resolve>>,
) -> Option<anyhow::Result<Client>> {
    if tls_config.is_none() && dns_resolver.is_none() {
        return None;
    }
    Some(build_outbound_client(OutboundClientConfig {
        pool_max_idle_per_host: Some(32),
        connect_timeout: Some(Duration::from_secs(5)),
        total_timeout: Some(Duration::from_secs(10)),
        redirect: OutboundRedirect::Deny,
        disable_proxy: true,
        tls: tls_config.clone(),
        tls_context: Some("Webhook"),
        dns_resolver,
        ..Default::default()
    }))
}

/// Build a pooled outbound reqwest client with an explicit timeout policy.
///
/// Centralizes the pool + redirect policy shared by plugin outbound clients,
/// while letting each caller pick its own connect timeout and decide whether to
/// impose a client-level total timeout.
///
/// - `connect_timeout`: TCP/TLS connect deadline.
/// - `total_timeout`: `Some(d)` sets reqwest's whole-request timeout; `None`
///   omits it — for streaming-body callers that enforce their own deadline.
///
/// Always sets `redirect::none` (redirect-following is an SSRF surface) and
/// `pool_max_idle_per_host(32)`. This is the request_mirror client shape; the
/// other outbound sites (task 03) will extend the signature with redirect /
/// proxy / pool parameters rather than call this unchanged.
pub fn build_pooled_outbound_client(
    connect_timeout: Duration,
    total_timeout: Option<Duration>,
) -> reqwest::Result<Client> {
    apply_outbound_policy(
        Client::builder(),
        OutboundClientConfig {
            pool_max_idle_per_host: Some(32),
            connect_timeout: Some(connect_timeout),
            total_timeout,
            redirect: OutboundRedirect::Deny,
            disable_proxy: true,
            ..Default::default()
        },
    )
    .expect("pooled outbound config has no TLS; apply_outbound_policy is infallible here")
    .build()
}

/// Canonical hop-by-hop headers that MUST NOT be forwarded to upstream services.
///
/// Per RFC 9110 §7.6.1 (which obsoletes RFC 2616 §13.5.1 and RFC 7230 §6.1).
/// The dynamic part of §7.6.1 — header names additionally nominated by the
/// `Connection:` header value of the current message — is handled by
/// [`connection_listed_hop_by_hop`] and [`is_hop_by_hop_or_listed`].
pub const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

/// Check whether `name` is one of the canonical hop-by-hop headers
/// (case-insensitive).
///
/// This only consults the static set in [`HOP_BY_HOP_HEADERS`]. To also honour
/// the per-message dynamic list nominated by the `Connection:` header value
/// (RFC 9110 §7.6.1), use [`is_hop_by_hop_or_listed`] together with
/// [`connection_listed_hop_by_hop`].
pub fn is_hop_by_hop(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    HOP_BY_HOP_HEADERS.contains(&lower.as_str())
}

/// Parse the value of the request's `Connection:` header into a deduplicated
/// list of lowercase connection-option names (RFC 9110 §7.6.1).
///
/// Each comma-separated token is trimmed and lowercased; empty tokens are
/// skipped. An intermediary MUST remove every header field whose name matches
/// one of these tokens before forwarding the message.
///
/// Callers that have no `Connection:` header should pass `""`, which returns
/// an empty list.
pub fn connection_listed_hop_by_hop(connection_value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in connection_value.split(',') {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_ascii_lowercase();
        if !out.iter().any(|s| s == &lower) {
            out.push(lower);
        }
    }
    out
}

/// Check whether `name` is hop-by-hop, considering both the canonical static
/// list ([`HOP_BY_HOP_HEADERS`]) and the dynamic list nominated by the request's
/// `Connection:` header value (RFC 9110 §7.6.1).
///
/// `dynamic_listed` should be the value returned by
/// [`connection_listed_hop_by_hop`] for the originating request.
pub fn is_hop_by_hop_or_listed(name: &str, dynamic_listed: &[String]) -> bool {
    let lower = name.to_ascii_lowercase();
    HOP_BY_HOP_HEADERS.contains(&lower.as_str()) || dynamic_listed.iter().any(|s| s == &lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hop_by_hop_detection() {
        // Exact lowercase match
        assert!(is_hop_by_hop("connection"));
        assert!(is_hop_by_hop("keep-alive"));
        assert!(is_hop_by_hop("transfer-encoding"));
        assert!(is_hop_by_hop("upgrade"));
        assert!(is_hop_by_hop("te"));
        assert!(is_hop_by_hop("trailers"));
        assert!(is_hop_by_hop("proxy-authenticate"));
        assert!(is_hop_by_hop("proxy-authorization"));

        // Case-insensitive
        assert!(is_hop_by_hop("Connection"));
        assert!(is_hop_by_hop("Keep-Alive"));
        assert!(is_hop_by_hop("TRANSFER-ENCODING"));
        assert!(is_hop_by_hop("Upgrade"));
        assert!(is_hop_by_hop("TE"));
        assert!(is_hop_by_hop("Proxy-Authenticate"));

        // Non hop-by-hop headers
        assert!(!is_hop_by_hop("content-type"));
        assert!(!is_hop_by_hop("authorization"));
        assert!(!is_hop_by_hop("host"));
        assert!(!is_hop_by_hop("x-forwarded-for"));
        assert!(!is_hop_by_hop("cookie"));
        assert!(!is_hop_by_hop("accept"));
    }

    #[test]
    fn test_connection_listed_hop_by_hop_parses_tokens() {
        // Single token
        assert_eq!(
            connection_listed_hop_by_hop("X-Sensitive"),
            vec!["x-sensitive".to_string()]
        );
        // Multiple tokens, mixed case, surrounding whitespace
        assert_eq!(
            connection_listed_hop_by_hop("X-A, X-B , close"),
            vec!["x-a".to_string(), "x-b".to_string(), "close".to_string()],
        );
        // Empty / whitespace tokens are skipped
        assert_eq!(connection_listed_hop_by_hop(",  ,X-Only,"), vec!["x-only".to_string()],);
        // Duplicates are deduplicated case-insensitively
        assert_eq!(
            connection_listed_hop_by_hop("X-Dup, x-dup, X-DUP"),
            vec!["x-dup".to_string()],
        );
        // Empty input
        assert!(connection_listed_hop_by_hop("").is_empty());
    }

    #[test]
    fn test_is_hop_by_hop_or_listed_combines_static_and_dynamic() {
        let listed = connection_listed_hop_by_hop("X-Sensitive, X-Token");
        // Static hop-by-hop matches regardless of dynamic set
        assert!(is_hop_by_hop_or_listed("Connection", &listed));
        assert!(is_hop_by_hop_or_listed("TRANSFER-ENCODING", &[]));
        // Dynamic-only names match when listed
        assert!(is_hop_by_hop_or_listed("x-sensitive", &listed));
        assert!(is_hop_by_hop_or_listed("X-Token", &listed));
        // Same names do not match when dynamic set is empty
        assert!(!is_hop_by_hop_or_listed("X-Sensitive", &[]));
        // Unrelated headers do not match
        assert!(!is_hop_by_hop_or_listed("Authorization", &listed));
        assert!(!is_hop_by_hop_or_listed("Cookie", &listed));
    }

    #[test]
    fn test_get_http_client_returns_same_instance() {
        let client1 = get_http_client();
        let client2 = get_http_client();
        // Both should point to the same instance
        assert!(std::ptr::eq(client1, client2));
    }
}

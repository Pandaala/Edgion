//! Shared HTTP client for plugins that need to call external services.
//!
//! Provides a global `reqwest::Client` singleton with connection pooling,
//! and utilities for hop-by-hop header filtering.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::core::plugins::edgion_plugins::common::http_client::get_http_client;
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

/// Global shared HTTP client with connection pooling.
///
/// Uses OnceLock for lazy initialization (consistent with RateLimit plugin pattern).
/// reqwest::Client is designed to be shared — it maintains an internal connection pool.
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

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
        Client::builder()
            .pool_max_idle_per_host(32)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build HTTP client")
    })
}

/// Hop-by-hop headers that MUST NOT be forwarded to external services.
/// Per RFC 2616 Section 13.5.1 and RFC 7230 Section 6.1
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

/// Check if a header name is a hop-by-hop header (case-insensitive).
pub fn is_hop_by_hop(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    HOP_BY_HOP_HEADERS.contains(&lower.as_str())
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
    fn test_get_http_client_returns_same_instance() {
        let client1 = get_http_client();
        let client2 = get_http_client();
        // Both should point to the same instance
        assert!(std::ptr::eq(client1, client2));
    }
}

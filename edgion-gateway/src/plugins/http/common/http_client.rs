//! Shared HTTP client for plugins that need to call external services.
//!
//! Provides process-wide `reqwest::Client` singletons with connection pooling:
//! the default client for ordinary plugin dependencies and a dedicated,
//! timeout-free RequestMirror client. It also provides utilities for hop-by-hop
//! header filtering.
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
//! - **Process-wide singletons**: `reqwest::Client` maintains an internal connection pool.
//!   The default and RequestMirror clients each share one pool across all of their consumers.
//! - **OnceLock lazy initialization**: Consistent with the project's pattern (e.g., RateLimitLocal).
//!   Each client is created only on first use.
//! - **No automatic redirects**: Auth-type requests should not follow redirects automatically.
//!   A 302 from an auth service typically indicates an authentication failure or redirect to login.
//! - **Timeout ownership**: The default client has fixed defensive timeouts. RequestMirror has no
//!   internal client timeout because its detached send owns one outer combined deadline.

use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;

use crate::link_sys::http_client::{
    build_outbound_client, build_outbound_client_with_tls_selection, OutboundClientConfig, OutboundRedirect,
};
use crate::link_sys::outbound_tls::{GlobalOutboundTlsSnapshot, ValidatedLocalTlsSelection};

/// Global shared HTTP client with connection pooling.
///
/// Uses OnceLock for lazy initialization; reqwest::Client is designed to be shared.
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static MIRROR_HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

fn build_client_or_exit(component: &'static str, config: OutboundClientConfig) -> Client {
    build_outbound_client(config).unwrap_or_else(|e| {
        tracing::error!(component, error = %e, "Failed to build HTTP client");
        std::process::exit(1);
    })
}

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
        build_client_or_exit(
            "http_client",
            OutboundClientConfig {
                pool_max_idle_per_host: Some(32),
                connect_timeout: Some(Duration::from_secs(5)),
                total_timeout: Some(Duration::from_secs(10)),
                redirect: OutboundRedirect::Deny,
                // Plugin dependency traffic must never inherit HTTP(S)_PROXY from
                // the gateway process. Proxy routing would bypass the canonical
                // dial-time egress resolver/classifier.
                disable_proxy: true,
                ..Default::default()
            },
        )
    })
}

/// Get the process-wide RequestMirror HTTP client.
///
/// RequestMirror owns its deadline around the complete detached send. This
/// client therefore has no internal connect or total timeout that could clip
/// the configured `connect_timeout + write_timeout` budget.
pub(crate) fn get_mirror_http_client() -> &'static Client {
    MIRROR_HTTP_CLIENT.get_or_init(|| {
        build_client_or_exit(
            "request_mirror_http_client",
            OutboundClientConfig {
                pool_max_idle_per_host: Some(32),
                connect_timeout: None,
                total_timeout: None,
                redirect: OutboundRedirect::Deny,
                disable_proxy: true,
                ..Default::default()
            },
        )
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
    let mut config = webhook_client_config(dns_resolver);
    config.tls = tls_config.clone();
    Some(build_outbound_client(config))
}

pub(crate) fn build_webhook_client_with_tls_selection(
    proof: &ValidatedLocalTlsSelection<'_>,
    defaults: &GlobalOutboundTlsSnapshot,
    dns_resolver: Option<std::sync::Arc<dyn reqwest::dns::Resolve>>,
) -> Option<anyhow::Result<Client>> {
    if proof.active_field().is_none() && dns_resolver.is_none() {
        return None;
    }
    Some(build_outbound_client_with_tls_selection(
        webhook_client_config(dns_resolver),
        proof,
        defaults,
    ))
}

fn webhook_client_config(dns_resolver: Option<std::sync::Arc<dyn reqwest::dns::Resolve>>) -> OutboundClientConfig {
    OutboundClientConfig {
        pool_max_idle_per_host: Some(32),
        connect_timeout: Some(Duration::from_secs(5)),
        total_timeout: Some(Duration::from_secs(10)),
        redirect: OutboundRedirect::Deny,
        disable_proxy: true,
        tls_context: Some("Webhook"),
        dns_resolver,
        ..Default::default()
    }
}

/// Canonical hop-by-hop headers that MUST NOT be forwarded to upstream services.
///
/// Per RFC 9110 §7.6.1 (which obsoletes RFC 2616 §13.5.1 and RFC 7230 §6.1).
/// The dynamic part of §7.6.1 — header names additionally nominated by the
/// `Connection:` header value of the current message — is handled by
/// [`connection_listed_hop_by_hop`] and [`is_hop_by_hop_or_listed`].
pub use edgion_resources::gwapi_types::HTTP_HOP_BY_HOP_HEADER_NAMES as HOP_BY_HOP_HEADERS;

/// Check whether `name` is one of the canonical hop-by-hop headers
/// (case-insensitive).
///
/// This only consults the static set in [`HOP_BY_HOP_HEADERS`]. To also honour
/// the per-message dynamic list nominated by the `Connection:` header value
/// (RFC 9110 §7.6.1), use [`is_hop_by_hop_or_listed`] together with
/// [`connection_listed_hop_by_hop`].
pub fn is_hop_by_hop(name: &str) -> bool {
    edgion_resources::gwapi_types::is_http_hop_by_hop_header_name(name)
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
    is_hop_by_hop(name) || dynamic_listed.iter().any(|s| s == &lower)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/http_client_test.rs"]
mod tests;

//! Shared reqwest client factory for outbound HTTP.
//!
//! Centralizes pool / timeout / redirect / proxy / TLS policy so every outbound
//! call site assembles its client the same way. `redirect::none` is the default
//! because a missing redirect policy is an SSRF surface.

use std::sync::OnceLock;
use std::time::Duration;

use edgion_resources::resources::common::TlsConfig;
use reqwest::{Client, ClientBuilder};

use super::tls::apply_resolved_outbound_tls;

/// Default decoded HTTP/2 response-header-list limit for canonical outbound clients.
pub const DEFAULT_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE: u32 = 16 * 1024;

/// Largest decoded HTTP/2 response-header-list limit accepted from process configuration.
pub const MAX_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE: u32 = 64 * 1024;

static OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE: OnceLock<u32> = OnceLock::new();

/// Install the process-wide decoded HTTP/2 response-header-list limit.
///
/// Call this once during process startup, before constructing any canonical
/// outbound client. Processes that do not configure an override use the
/// explicit 16 KiB default rather than inheriting reqwest's mutable default.
pub fn configure_outbound_http2_max_header_list_size(size: u32) -> anyhow::Result<()> {
    validate_outbound_http2_max_header_list_size(size)?;
    OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE
        .set(size)
        .map_err(|_| anyhow::anyhow!("outbound HTTP/2 response header list size was already initialized"))
}

fn validate_outbound_http2_max_header_list_size(size: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        (DEFAULT_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE..=MAX_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE).contains(&size),
        "outbound HTTP/2 response header list size must be between {} and {} bytes, got {}",
        DEFAULT_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE,
        MAX_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE,
        size
    );
    Ok(())
}

fn outbound_http2_max_header_list_size() -> u32 {
    *OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE.get_or_init(|| DEFAULT_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE)
}

/// Redirect policy for an outbound client.
#[derive(Default)]
pub enum OutboundRedirect {
    /// `reqwest::redirect::Policy::none()` — follow no redirects. The default,
    /// because forgetting a redirect policy is an SSRF surface.
    #[default]
    Deny,
    /// `reqwest::redirect::Policy::limited(n)` — follow up to `n` redirects.
    Limit(usize),
    /// Caller-supplied policy (e.g. the DSL SSRF-guarded redirect closure).
    Custom(reqwest::redirect::Policy),
}

/// Transport-policy knobs for building an outbound `reqwest::Client`.
///
/// Consumed by value by [`apply_outbound_policy`] / [`build_outbound_client`]
/// because [`OutboundRedirect::Custom`] holds a non-`Clone`
/// `reqwest::redirect::Policy`. Construct it inline at build time; it is not
/// meant to be stored or reused.
#[derive(Default)]
pub struct OutboundClientConfig {
    /// `pool_max_idle_per_host`. `None` leaves reqwest's default.
    pub pool_max_idle_per_host: Option<usize>,
    /// `pool_idle_timeout`. `None` leaves reqwest's default.
    pub pool_idle_timeout: Option<Duration>,
    /// `connect_timeout`. `None` leaves it unset.
    pub connect_timeout: Option<Duration>,
    /// Whole-request `timeout`. `None` leaves it unset (for callers that enforce
    /// a per-request deadline instead).
    pub total_timeout: Option<Duration>,
    /// Redirect policy. Defaults to [`OutboundRedirect::Deny`].
    pub redirect: OutboundRedirect,
    /// `true` → `no_proxy()` (ignore HTTP(S)_PROXY / NO_PROXY env).
    pub disable_proxy: bool,
    /// Optional TLS config (CA / client identity / gated `verify=false`),
    /// applied through the shared [`super::tls`] helpers. The caller is
    /// responsible for the `tls.enabled` gate — pass `None` when TLS is disabled.
    pub tls: Option<TlsConfig>,
    /// Fully prepared rustls configuration for shared custom-verifier support.
    /// Mutually exclusive with `tls`; resolution remains the caller's responsibility.
    pub preconfigured_tls: Option<rustls::ClientConfig>,
    /// Label used in TLS error messages (e.g. "Webhook", "Elasticsearch").
    /// `None` → "OutboundClient".
    pub tls_context: Option<&'static str>,
    /// Custom DNS resolver. `None` leaves reqwest's default (GAI).
    ///
    /// The gateway passes an `EgressResolver` here so that SSRF classification runs
    /// on the exact addresses reqwest connects to.
    pub dns_resolver: Option<std::sync::Arc<dyn reqwest::dns::Resolve>>,
}

/// Apply `cfg` to `builder`. Consumed by value — see [`OutboundClientConfig`].
///
/// Mirrors the existing `apply_*(builder) -> builder` idiom in [`super::tls`], so
/// callers that need extra builder customization (e.g. `default_headers`) can
/// chain onto the returned builder before `build()`.
pub fn apply_outbound_policy(builder: ClientBuilder, cfg: OutboundClientConfig) -> anyhow::Result<ClientBuilder> {
    let mut builder = builder;
    anyhow::ensure!(
        cfg.tls.is_none() || cfg.preconfigured_tls.is_none(),
        "OutboundClient: tls and preconfigured_tls are mutually exclusive"
    );
    if let Some(n) = cfg.pool_max_idle_per_host {
        builder = builder.pool_max_idle_per_host(n);
    }
    if let Some(d) = cfg.pool_idle_timeout {
        builder = builder.pool_idle_timeout(d);
    }
    if let Some(d) = cfg.connect_timeout {
        builder = builder.connect_timeout(d);
    }
    if let Some(d) = cfg.total_timeout {
        builder = builder.timeout(d);
    }
    builder = builder.http2_max_header_list_size(outbound_http2_max_header_list_size());
    builder = match cfg.redirect {
        OutboundRedirect::Deny => builder.redirect(reqwest::redirect::Policy::none()),
        OutboundRedirect::Limit(n) => builder.redirect(reqwest::redirect::Policy::limited(n)),
        OutboundRedirect::Custom(p) => builder.redirect(p),
    };
    if cfg.disable_proxy {
        builder = builder.no_proxy();
    }
    if let Some(resolver) = cfg.dns_resolver {
        // `dns_resolver2` (not `dns_resolver`) — the latter's `R: Sized` bound
        // rejects `Arc<dyn Resolve>`.
        builder = builder.dns_resolver2(resolver);
    }
    if let Some(tls) = cfg.tls.as_ref() {
        let ctx = cfg.tls_context.unwrap_or("OutboundClient");
        builder = apply_resolved_outbound_tls(builder, tls, ctx)?;
    }
    if let Some(tls) = cfg.preconfigured_tls {
        builder = builder.use_preconfigured_tls(tls);
    }
    Ok(builder)
}

/// Build a `reqwest::Client` from `cfg`. Convenience over [`apply_outbound_policy`].
pub fn build_outbound_client(cfg: OutboundClientConfig) -> anyhow::Result<Client> {
    Ok(apply_outbound_policy(Client::builder(), cfg)?.build()?)
}

#[cfg(test)]
#[path = "../../tests/unit/link_sys_core/http_client_test.rs"]
mod tests;

//! Shared reqwest client factory for outbound HTTP.
//!
//! Centralizes pool / timeout / redirect / proxy / TLS policy so every outbound
//! call site assembles its client the same way. `redirect::none` is the default
//! because a missing redirect policy is an SSRF surface.

use std::time::Duration;

use edgion_resources::resources::common::TlsConfig;
use reqwest::{Client, ClientBuilder};

use super::tls::{apply_insecure_skip_verify, apply_to_reqwest_builder};

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
    /// Optional TLS config (CA / client identity / gated `insecure_skip_verify`),
    /// applied through the shared [`super::tls`] helpers. The caller is
    /// responsible for the `tls.enabled` gate — pass `None` when TLS is disabled.
    pub tls: Option<TlsConfig>,
    /// Label used in TLS error messages (e.g. "Webhook", "Elasticsearch").
    /// `None` → "OutboundClient".
    pub tls_context: Option<&'static str>,
    /// RAW, UNGATED `danger_accept_invalid_certs`. Unlike `tls`'s
    /// `insecure_skip_verify` (which is refused in production), this is NOT gated.
    /// Exists ONLY to preserve the legacy OIDC `ssl_verify=false` behavior; do not
    /// use for new code.
    pub danger_accept_invalid_certs: bool,
}

/// Apply `cfg` to `builder`. Consumed by value — see [`OutboundClientConfig`].
///
/// Mirrors the existing `apply_*(builder) -> builder` idiom in [`super::tls`], so
/// callers that need extra builder customization (e.g. `default_headers`) can
/// chain onto the returned builder before `build()`.
pub fn apply_outbound_policy(mut builder: ClientBuilder, cfg: OutboundClientConfig) -> anyhow::Result<ClientBuilder> {
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
    builder = match cfg.redirect {
        OutboundRedirect::Deny => builder.redirect(reqwest::redirect::Policy::none()),
        OutboundRedirect::Limit(n) => builder.redirect(reqwest::redirect::Policy::limited(n)),
        OutboundRedirect::Custom(p) => builder.redirect(p),
    };
    if cfg.disable_proxy {
        builder = builder.no_proxy();
    }
    if cfg.danger_accept_invalid_certs {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(tls) = cfg.tls.as_ref() {
        let ctx = cfg.tls_context.unwrap_or("OutboundClient");
        builder = apply_insecure_skip_verify(builder, tls, ctx)?;
        builder = apply_to_reqwest_builder(builder, tls, ctx)?;
    }
    Ok(builder)
}

/// Build a `reqwest::Client` from `cfg`. Convenience over [`apply_outbound_policy`].
pub fn build_outbound_client(cfg: OutboundClientConfig) -> anyhow::Result<Client> {
    Ok(apply_outbound_policy(Client::builder(), cfg)?.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::TlsConfig;

    #[test]
    fn default_redirect_is_deny() {
        assert!(matches!(OutboundRedirect::default(), OutboundRedirect::Deny));
    }

    #[test]
    fn build_default_client_succeeds() {
        assert!(build_outbound_client(OutboundClientConfig::default()).is_ok());
    }

    #[test]
    fn build_with_all_transport_knobs_succeeds() {
        let cfg = OutboundClientConfig {
            pool_max_idle_per_host: Some(5),
            pool_idle_timeout: Some(Duration::from_secs(10)),
            connect_timeout: Some(Duration::from_secs(3)),
            total_timeout: Some(Duration::from_secs(10)),
            redirect: OutboundRedirect::Limit(10),
            disable_proxy: true,
            danger_accept_invalid_certs: true,
            ..Default::default()
        };
        assert!(build_outbound_client(cfg).is_ok());
    }

    #[test]
    fn custom_redirect_policy_is_accepted() {
        let cfg = OutboundClientConfig {
            redirect: OutboundRedirect::Custom(reqwest::redirect::Policy::limited(3)),
            ..Default::default()
        };
        assert!(build_outbound_client(cfg).is_ok());
    }

    #[test]
    fn insecure_tls_is_refused_in_production() {
        let _guard = crate::link_sys_core::tls::ENV_TEST_MUTEX.lock();
        let prev = std::env::var("EDGION_ENV").ok();
        std::env::set_var("EDGION_ENV", "production");

        let cfg = OutboundClientConfig {
            tls: Some(TlsConfig {
                enabled: true,
                certs: None,
                insecure_skip_verify: Some(true),
            }),
            tls_context: Some("TestProvider"),
            ..Default::default()
        };
        let result = build_outbound_client(cfg);

        match prev {
            Some(v) => std::env::set_var("EDGION_ENV", v),
            None => std::env::remove_var("EDGION_ENV"),
        }
        assert!(
            result.is_err(),
            "insecure_skip_verify must be refused when EDGION_ENV=production"
        );
    }
}

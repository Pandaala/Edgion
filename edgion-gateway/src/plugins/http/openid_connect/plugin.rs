//! OpenID Connect plugin implementation.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use pingora_http::ResponseHeader;
use rand::RngCore;
use reqwest::Url;
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::error::{map_jwt_decode_error_typed, OidcVerifyError};
use super::jwks_provider::{DiscoveryDoc, DiscoveryValidator, JwksProvider, JwksProviderConfig, JwksProviderError};
use super::jwt_common::{resolve_algorithm_policy, select_jwk, validate_token_alg, JwkSelectError};
use super::refresh_flight_registry::RefreshFlightRegistry;
use crate::cache::{LogicalWeight, ShardedLruCache, TinyUfoTtlCache};
use crate::cli::config::{
    get_default_lru_cache_capacity, get_default_lru_cache_shards, get_refresh_singleflight_max_registered_keys,
    resolve_lru_cache_limits, resolve_tiny_ufo_ttl_cache_limits,
};
use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
use crate::plugins::http::common::auth_common::{
    send_gateway_auth_error_response, set_claims_headers as set_common_claims_headers, Claims,
};
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::tls::validation::OutboundSanVerifier;
use edgion_common::link_sys_core::outbound_tls::{resolve_outbound_tls, EffectiveCa, ResolvedOutboundTls};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::path_normalization::normalize_config_path;
use edgion_resources::resources::backend_tls_policy::SubjectAltName;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::edgion_plugins::{
    EndpointAuthMethod, OpenidConnectConfig, UnauthAction, VerificationMode,
};

type OidcError = Box<dyn std::error::Error + Send + Sync>;
type OidcResult<T> = Result<T, OidcError>;
type VerifyResult<T> = Result<T, OidcVerifyError>;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct DiscoveryDocument {
    pub(super) issuer: String,
    pub(super) jwks_uri: String,
    #[serde(default)]
    pub(super) authorization_endpoint: Option<String>,
    #[serde(default)]
    pub(super) token_endpoint: Option<String>,
    #[serde(default)]
    pub(super) introspection_endpoint: Option<String>,
    #[serde(default)]
    pub(super) userinfo_endpoint: Option<String>,
    #[serde(default)]
    pub(super) end_session_endpoint: Option<String>,
    #[serde(default)]
    pub(super) revocation_endpoint: Option<String>,
}

impl DiscoveryDoc for DiscoveryDocument {
    fn issuer(&self) -> &str {
        &self.issuer
    }
    fn jwks_uri(&self) -> &str {
        &self.jwks_uri
    }
}

/// Build a rustls `ClientConfig` for OIDC outbound HTTPS with SAN pinning enforced.
///
/// Used only when the operator configured `tls.validation.subjectAltNames`.
/// Bypasses reqwest's high-level `add_root_certificate` / `identity` helpers
/// because they don't allow attaching a custom `ServerCertVerifier`.
///
/// Mirrors `build_ldap_rustls_config` in `ldap_auth/plugin.rs`, with two OIDC
/// additions: ALPN protocols (`h2`, `http/1.1`) for HTTPS+HTTP/2 negotiation,
/// and a return type tailored for `reqwest::ClientBuilder::use_preconfigured_tls`.
fn build_oidc_rustls_config_with_san(
    resolved: &ResolvedOutboundTls,
    san_policy: Vec<SubjectAltName>,
) -> Result<ClientConfig, String> {
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("OpenidConnect TLS: rustls provider error: {e}"))?;

    // Root store
    let mut roots = RootCertStore::empty();
    match &resolved.ca {
        EffectiveCa::System => {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        EffectiveCa::Custom(bundles) => {
            let mut added = 0usize;
            for bundle in bundles {
                for cert in CertificateDer::pem_slice_iter(bundle) {
                    let cert = cert.map_err(|e| format!("OpenidConnect TLS: invalid CA PEM: {e}"))?;
                    roots
                        .add(cert)
                        .map_err(|e| format!("OpenidConnect TLS: add CA failed: {e}"))?;
                    added += 1;
                }
            }
            if added == 0 {
                return Err("OpenidConnect TLS: CA PEM contained no valid certificates".to_string());
            }
        }
    }

    // Wrap webpki verifier with OutboundSanVerifier
    let webpki = WebPkiServerVerifier::builder_with_provider(std::sync::Arc::new(roots), provider)
        .build()
        .map_err(|e| format!("OpenidConnect TLS: webpki verifier build error: {e}"))?;
    let san_verifier = std::sync::Arc::new(OutboundSanVerifier::new(webpki, san_policy));
    let builder = builder.dangerous().with_custom_certificate_verifier(san_verifier);

    // Client identity (mTLS) — same pattern as ldap_auth/plugin.rs
    let mut config = match &resolved.client_identity {
        Some((cert_pem, key_pem)) => {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("OpenidConnect TLS: invalid client cert PEM: {e}"))?;
            if certs.is_empty() {
                return Err("OpenidConnect TLS: client cert PEM contained no certificates".to_string());
            }
            let key = PrivateKeyDer::from_pem_slice(key_pem.as_slice())
                .map_err(|e| format!("OpenidConnect TLS: invalid client key PEM: {e}"))?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| format!("OpenidConnect TLS: client auth cert error: {e}"))?
        }
        None => builder.with_no_client_auth(),
    };

    // ALPN — OIDC over HTTPS may negotiate h2 (reqwest does so by default)
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(config)
}

/// Build a config-aware discovery validator that fails fetch when a required
/// endpoint is missing for the configured plugin behavior.
fn build_discovery_validator(config: &OpenidConnectConfig) -> DiscoveryValidator<DiscoveryDocument> {
    // Snapshot config flags into the closure so it stays Send + Sync + 'static.
    let unauth_action = config.unauth_action;
    let needs_token_endpoint = config.unauth_action == UnauthAction::Auth
        || config.verification_mode != VerificationMode::JwksOnly
        || config.renew_access_token_on_expiry;
    let needs_userinfo = config.set_userinfo_header;

    Arc::new(move |doc: &DiscoveryDocument| {
        if unauth_action == UnauthAction::Auth
            && doc
                .authorization_endpoint
                .as_deref()
                .filter(|v| !v.is_empty())
                .is_none()
        {
            return Err("authorization_endpoint required when unauthAction=Auth".into());
        }
        if needs_token_endpoint && doc.token_endpoint.as_deref().filter(|v| !v.is_empty()).is_none() {
            return Err("token_endpoint required for the configured verification/refresh mode".into());
        }
        if needs_userinfo && doc.userinfo_endpoint.as_deref().filter(|v| !v.is_empty()).is_none() {
            return Err("userinfo_endpoint required when set_userinfo_header=true".into());
        }
        // introspection_endpoint can be overridden at the plugin config layer
        // (`config.introspection_endpoint`), so we do not enforce it at fetch time.
        Ok(())
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthorizationStateCookie {
    state: String,
    original_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_verifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OidcSessionCookie {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    session_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    access_token: String,
    created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    #[allow(dead_code)]
    #[serde(default)]
    id_token: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[allow(dead_code)]
    #[serde(default)]
    token_type: Option<String>,
}

#[derive(Debug, Clone)]
struct IntrospectionCacheEntry {
    claims: Value,
}

#[derive(Debug, Clone)]
struct AccessTokenCacheEntry {
    token: String,
}

impl crate::cache::tiny_ufo::sealed::Sealed for IntrospectionCacheEntry {}
impl LogicalWeight for IntrospectionCacheEntry {
    fn logical_weight(&self) -> usize {
        self.claims.logical_weight()
    }
}

/// OpenID Connect request filter plugin.
pub struct OpenidConnect {
    name: String,
    config: OpenidConnectConfig,
    callback_path: String,
    #[allow(dead_code)]
    plugin_namespace: String,
    /// Per-instance outbound HTTP client. Built at construction with the plugin's
    /// resolved TLS policy (custom CA, mTLS identity, verify gate). Construction
    /// fails closed if any declared TLS policy cannot be applied.
    pub(super) client: Arc<reqwest::Client>,
    pub(super) jwks_provider: Arc<JwksProvider<DiscoveryDocument>>,
    introspection_cache: TinyUfoTtlCache<String, IntrospectionCacheEntry>,
    access_token_cache: ShardedLruCache<String, AccessTokenCacheEntry>,
    refresh_singleflight: RefreshFlightRegistry,
    refresh_singleflight_results: ShardedLruCache<String, OidcSessionCookie>,
}

impl OpenidConnect {
    #[cfg(test)]
    pub fn new(config: &OpenidConnectConfig, plugin_namespace: String) -> Self {
        Self::try_new(config, plugin_namespace).expect("test OIDC configuration must build")
    }

    pub fn try_new(config: &OpenidConnectConfig, plugin_namespace: String) -> Result<Self, String> {
        // Normalize request-path-matched config fields at construction time so that
        // config-side and request-side paths inhabit the same normalized space.
        // See docs/superpowers/specs/2026-04-26-uri-normalization-design.md §3.5.
        let mut config = config.clone();
        let path_cfg = edgion_resources::path_normalization::get_path_normalization_config();
        let normalized_logout_path = normalize_config_path(&config.logout_path, path_cfg);
        if normalized_logout_path != config.logout_path {
            tracing::info!(
                "openid_connect: config path normalized: {:?} -> {:?}",
                config.logout_path,
                normalized_logout_path
            );
            config.logout_path = normalized_logout_path;
        }
        let configured_callback = config
            .redirect_uri
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("/.edgion/oidc/callback");
        let raw_callback_path = if configured_callback.starts_with('/') {
            configured_callback.to_string()
        } else if let Ok(url) = Url::parse(configured_callback) {
            url.path().to_string()
        } else {
            "/.edgion/oidc/callback".to_string()
        };
        let callback_path = normalize_config_path(&raw_callback_path, path_cfg);

        let introspection_limits = resolve_tiny_ufo_ttl_cache_limits(&config.introspection.tiny_ufo_ttl_cache)?;
        let (access_token_capacity, access_token_shards) = resolve_lru_cache_limits(&config.access_token.lru_cache)?;
        let shared_lru_capacity = get_default_lru_cache_capacity();
        let shared_lru_shards = get_default_lru_cache_shards();

        // Build the per-instance outbound client before the jwks_provider block so the
        // client Arc can be cloned into both the provider and stored on Self.
        let outbound_client: Arc<reqwest::Client> = {
            let tls_for_client = TlsConfig {
                enabled: true,
                outbound: config.tls.clone(),
                resolved_ca_certificates: config.resolved_ca_certificates.clone(),
                resolved_client_certificate: config.resolved_client_certificate.clone(),
            };

            // Phase 2: if SAN pinning is configured (validation.subjectAltNames),
            // build reqwest with use_preconfigured_tls(rustls::ClientConfig) instead
            // of the high-level helpers (which can't attach a ServerCertVerifier).
            let san_policy: Vec<SubjectAltName> = tls_for_client
                .outbound
                .validation
                .as_ref()
                .and_then(|v| v.subject_alt_names.clone())
                .unwrap_or_default();

            if !san_policy.is_empty() {
                let resolved = resolve_outbound_tls(
                    &tls_for_client.outbound,
                    tls_for_client.resolved_ca_certificates.as_deref(),
                    tls_for_client.resolved_client_certificate.as_ref(),
                    true,
                    "OpenidConnect",
                )
                .map_err(|error| {
                    tracing::warn!(
                        component = "openid_connect",
                        error = ?error,
                        "outbound TLS resolution failed; OIDC plugin construction rejected"
                    );
                    "openid_connect outbound TLS resolution failed".to_string()
                })?;
                let rustls_config = build_oidc_rustls_config_with_san(&resolved, san_policy).map_err(|error| {
                    tracing::warn!(
                        component = "openid_connect",
                        error = %error,
                        "SAN-pinning TLS config build failed; OIDC plugin construction rejected"
                    );
                    "openid_connect SAN-pinning TLS config build failed".to_string()
                })?;
                let client = reqwest::ClientBuilder::new()
                    .pool_max_idle_per_host(32)
                    .connect_timeout(Duration::from_secs(5))
                    .timeout(Duration::from_secs(config.timeout))
                    .redirect(reqwest::redirect::Policy::none())
                    .use_preconfigured_tls(rustls_config)
                    .build()
                    .map_err(|error| {
                        tracing::warn!(
                            component = "openid_connect",
                            error = %error,
                            "SAN-pinning HTTP client build failed; OIDC plugin construction rejected"
                        );
                        "openid_connect SAN-pinning HTTP client build failed".to_string()
                    })?;
                Arc::new(client)
            } else {
                let client = build_outbound_client(OutboundClientConfig {
                    pool_max_idle_per_host: Some(32),
                    connect_timeout: Some(Duration::from_secs(5)),
                    total_timeout: Some(Duration::from_secs(config.timeout)),
                    redirect: OutboundRedirect::Deny,
                    tls: Some(tls_for_client),
                    tls_context: Some("OpenidConnect"),
                    ..Default::default()
                })
                .map_err(|error| {
                    tracing::warn!(
                        component = "openid_connect",
                        error = %error,
                        "outbound HTTP client build failed; OIDC plugin construction rejected"
                    );
                    "openid_connect outbound HTTP client build failed".to_string()
                })?;
                Arc::new(client)
            }
        };

        let jwks_provider = {
            let client = outbound_client.clone();
            let provider_config = JwksProviderConfig {
                discovery_url: config.discovery.clone(),
                jwks_cache_ttl: Duration::from_secs(config.jwks_cache_ttl),
                jwks_min_refresh_interval: Duration::from_secs(config.jwks_min_refresh_interval),
                discovery_max_response_bytes: config.discovery_max_response_bytes,
                jwks_max_response_bytes: config.jwks_max_response_bytes,
                request_timeout: Duration::from_secs(config.timeout),
                component_label: "openid_connect",
            };
            JwksProvider::new(provider_config, client, build_discovery_validator(&config))
        };

        // Pre-warm discovery cache in the background. The gateway logout path reads
        // cached discovery only (no fetch on the request) and silently degrades RP-
        // Initiated Logout / token revocation when the cache is cold. Pre-warming on
        // construction closes the "first request after gateway restart is /logout"
        // gap without adding an HTTP RTT to the request path.
        //
        // Gated out under `cfg(test)` so unit tests do not fan out real DNS/HTTPS
        // probes against placeholder discovery URLs. Skipped when the config is
        // already known-invalid (the request filter rejects the request anyway, and
        // a misconfigured discovery URL would just burn ~5 min of background retries
        // per resync). The task uses a Weak ref so it self-exits as soon as the
        // plugin instance is dropped (e.g., on EdgionPlugins resync), bounding
        // worst-case task fan-out under repeated reconstruction.
        #[cfg(not(test))]
        if config.get_validation_error().is_none() {
            spawn_discovery_prewarm(
                Arc::downgrade(&jwks_provider),
                plugin_namespace.clone(),
                config.discovery.clone(),
            );
        }

        Ok(Self {
            name: "OpenidConnect".to_string(),
            config,
            callback_path,
            plugin_namespace,
            client: outbound_client,
            jwks_provider,
            introspection_cache: TinyUfoTtlCache::new(introspection_limits).map_err(|e| e.to_string())?,
            access_token_cache: ShardedLruCache::new(access_token_capacity, access_token_shards),
            refresh_singleflight: RefreshFlightRegistry::new(get_refresh_singleflight_max_registered_keys())?,
            refresh_singleflight_results: ShardedLruCache::new(shared_lru_capacity, shared_lru_shards),
        })
    }
}

/// Background discovery cache pre-warm. Bounded retries (immediate + ~5 min of
/// exponential backoff) so a slow-starting IdP eventually populates the cache.
/// The Weak ref drops with the plugin: once the owning OpenidConnect is gone
/// (config resync, plugin removal), the next upgrade fails and the task exits.
///
/// This task is also the only place allowed to log a successful discovery load:
/// `JwksProvider::get_or_fetch_discovery` is request-path reachable (lazy fetch
/// on a cold cache), so the emission lives here on the provably cold caller
/// rather than inside the provider. A discovery load that happens only via the
/// request-path lazy fetch therefore logs nothing — the prewarm-exhausted warn
/// below already told the operator the cache was left cold.
#[cfg(not(test))]
fn spawn_discovery_prewarm(
    jwks_provider: std::sync::Weak<JwksProvider<DiscoveryDocument>>,
    plugin_namespace: String,
    discovery_url: String,
) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        // Cumulative wall time after each delay: 1, 3, 7, 15, 31, 63, 123, 183, 243, 303 s.
        const RETRY_DELAYS_SECS: [u64; 10] = [1, 2, 4, 8, 16, 32, 60, 60, 60, 60];
        let log_loaded = |doc: &DiscoveryDocument| {
            tracing::info!(
                component = "openid_connect",
                issuer = %doc.issuer,
                jwks_uri = %doc.jwks_uri,
                "OIDC discovery document loaded"
            );
        };
        let Some(provider) = jwks_provider.upgrade() else {
            return;
        };
        if let Ok(doc) = provider.get_or_fetch_discovery().await {
            log_loaded(&doc);
            return;
        }
        // Release the strong ref before sleeping so the plugin Arc can drop
        // (e.g., on EdgionPlugins resync) and the next upgrade short-circuits.
        drop(provider);
        for delay_secs in RETRY_DELAYS_SECS {
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            let Some(provider) = jwks_provider.upgrade() else {
                return;
            };
            if let Ok(doc) = provider.get_or_fetch_discovery().await {
                log_loaded(&doc);
                return;
            }
        }
        // All retries exhausted: surface a single signal so operators can spot a
        // misconfigured discovery URL or a long-down IdP. After this point the
        // first real request still triggers a lazy fetch via get_or_fetch_discovery.
        tracing::warn!(
            component = "openid_connect",
            plugin_namespace = %plugin_namespace,
            discovery_url = %discovery_url,
            "OIDC discovery prewarm exhausted retries; cache remains cold until first request \
             triggers lazy fetch. If outbound TLS SAN pinning is configured, check the \
             edgion_gateway_outbound_tls_san_mismatch_total counter and the access log for \
             OutboundTlsSanMismatch (the discovery fetch itself is request-path reachable and \
             therefore logs nothing)"
        );
    });
}

#[path = "callback.rs"]
mod callback;
#[path = "config_validation.rs"]
mod config_validation;
#[path = "discovery_jwks.rs"]
mod discovery_jwks;
#[path = "logout.rs"]
mod logout;
#[path = "request_filter_impl.rs"]
mod request_filter_impl;
#[path = "session_cookie.rs"]
mod session_cookie;
#[path = "token_exchange.rs"]
mod token_exchange;
#[path = "token_verify.rs"]
mod token_verify;
#[path = "userinfo.rs"]
mod userinfo;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

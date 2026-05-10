//! OpenID Connect plugin implementation (Phase 1: Bearer + Discovery + JWKS).

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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use super::error::{map_jwt_decode_error_typed, OidcVerifyError};
use crate::core::common::auth::jwks_provider::{DiscoveryDoc, DiscoveryValidator, JwksProvider, JwksProviderConfig};
use crate::core::common::jwt_common::{resolve_algorithm_policy, select_jwk, validate_token_alg, JwkSelectError};
use crate::core::gateway::cache::ShardedLruCache;
use crate::core::gateway::cli::config::{get_default_lru_cache_capacity, get_default_lru_cache_shards};
use crate::core::gateway::path_normalization::normalize_config_path;
use crate::core::gateway::plugins::http::common::auth_common::{
    send_auth_error_response, set_claims_headers as set_common_claims_headers, Claims,
};
use crate::core::gateway::plugins::http::common::http_client::get_http_client_with_ssl_verify;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{
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

/// Build a config-aware discovery validator that fails fetch when a required
/// endpoint is missing for the configured plugin behavior. Closes g3-009 by
/// shifting "missing endpoint" failures from request-time 502 to fetch-time.
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
    // expires_at removed — TTL owned by LocalLruCache
}

#[derive(Debug, Clone)]
struct AccessTokenCacheEntry {
    token: String,
    _expires_at: Option<u64>,
}

/// OpenID Connect request filter plugin.
pub struct OpenidConnect {
    name: String,
    config: OpenidConnectConfig,
    #[allow(dead_code)]
    plugin_namespace: String,
    pub(super) jwks_provider: Arc<JwksProvider<DiscoveryDocument>>,
    introspection_cache: Arc<ShardedLruCache<String, IntrospectionCacheEntry>>,
    access_token_cache: Arc<ShardedLruCache<String, AccessTokenCacheEntry>>,
    // Per-key async mutex for the refresh singleflight critical section. Held across `await`,
    // so the value type must be `tokio::sync::Mutex`. LRU semantics give bounded memory and
    // age-based eviction. Two non-fatal degradations vs. an atomic `entry().or_insert_with()`:
    // (1) a TOCTOU between get/insert can briefly produce two locks for the same key, and
    // (2) LRU eviction can orphan a lock while a waiter is still parked on it. Both collapse
    // to at most one extra refresh round-trip for that key — the post-lock
    // `try_get_recent_refresh_result` check still de-dupes whenever the leader's result has
    // landed before the racing waiter finishes its lock acquire; the worst case is leader
    // and racing task both completing a refresh in parallel. No session-state corruption.
    refresh_singleflight_locks: Arc<ShardedLruCache<String, Arc<Mutex<()>>>>,
    refresh_singleflight_results: Arc<ShardedLruCache<String, OidcSessionCookie>>,
}

impl OpenidConnect {
    pub fn new(config: &OpenidConnectConfig, plugin_namespace: String) -> Self {
        // Normalize request-path-matched config fields at construction time so that
        // config-side and request-side paths inhabit the same normalized space.
        // See docs/superpowers/specs/2026-04-26-uri-normalization-design.md §3.5.
        let mut config = config.clone();
        let path_cfg = crate::core::gateway::cli::config::get_path_normalization_config();
        let normalized_logout_path = normalize_config_path(&config.logout_path, path_cfg);
        if normalized_logout_path != config.logout_path {
            tracing::info!(
                "openid_connect: config path normalized: {:?} -> {:?}",
                config.logout_path,
                normalized_logout_path
            );
            config.logout_path = normalized_logout_path;
        }

        // Extract cache parameters before config is moved into Self.
        let introspection_capacity = if config.introspection.lru_cache.capacity == 0 {
            get_default_lru_cache_capacity()
        } else {
            config.introspection.lru_cache.capacity
        };
        let introspection_shards = if config.introspection.lru_cache.shards > 0 {
            config.introspection.lru_cache.shards
        } else {
            get_default_lru_cache_shards()
        };
        let access_token_capacity = if config.access_token.lru_cache.capacity == 0 {
            get_default_lru_cache_capacity()
        } else {
            config.access_token.lru_cache.capacity
        };
        let access_token_shards = if config.access_token.lru_cache.shards > 0 {
            config.access_token.lru_cache.shards
        } else {
            get_default_lru_cache_shards()
        };

        let jwks_provider = {
            let client = Arc::new(get_http_client_with_ssl_verify(config.ssl_verify).clone());
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
        // gap without adding an HTTP RTT to the request path. (g3-007)
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

        Self {
            name: "OpenidConnect".to_string(),
            config,
            plugin_namespace,
            jwks_provider,
            introspection_cache: Arc::new(ShardedLruCache::new(introspection_capacity, introspection_shards)),
            access_token_cache: Arc::new(ShardedLruCache::new(access_token_capacity, access_token_shards)),
            refresh_singleflight_locks: Arc::new(ShardedLruCache::new(
                get_default_lru_cache_capacity(),
                get_default_lru_cache_shards(),
            )),
            refresh_singleflight_results: Arc::new(ShardedLruCache::new(
                get_default_lru_cache_capacity(),
                get_default_lru_cache_shards(),
            )),
        }
    }
}

/// Background discovery cache pre-warm. Bounded retries (immediate + ~5 min of
/// exponential backoff) so a slow-starting IdP eventually populates the cache.
/// The Weak ref drops with the plugin: once the owning OpenidConnect is gone
/// (config resync, plugin removal), the next upgrade fails and the task exits.
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
        let Some(provider) = jwks_provider.upgrade() else {
            return;
        };
        if provider.get_or_fetch_discovery().await.is_ok() {
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
            if provider.get_or_fetch_discovery().await.is_ok() {
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
            "OIDC discovery prewarm exhausted retries; cache remains cold until first request triggers lazy fetch"
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

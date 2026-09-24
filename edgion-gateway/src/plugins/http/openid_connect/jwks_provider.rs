//! Generic JWKS + OIDC discovery cache provider.
//!
//! A single instance maintains both the discovery document and the JWKS cache,
//! **with unified stale-while-revalidate semantics**: when the JWKS endpoint is
//! unreachable and a cached set exists locally, the cached set is returned
//! (flagged `served_stale`) instead of propagating the fetch error.
//!
//! Both entry points are reachable from the request path, so this file is
//! `tracing`-free (Iron rule 3). Failures surface through
//! `edgion_gateway_outbound_tls_san_mismatch_total`, the caller's `PluginLog` /
//! `EdgionErrorCode`, and the `served_stale` dye outcome recorded by
//! `discovery_jwks.rs`. The one genuinely cold signal — "discovery document
//! loaded" — is emitted by the background prewarm task in `plugin.rs`.
//! `JwksProviderConfig` parameterizes TTL / min refresh interval / response byte
//! limit / timeout / component label. The discovery document shape is determined
//! by the generic `D: DiscoveryDoc` -- the OIDC gateway plugin uses an extended
//! variant (including 5 optional endpoints).

use jsonwebtoken::jwk::JwkSet;
#[cfg(test)]
use parking_lot::Mutex as ScriptMutex;
use reqwest::Client;
use serde::de::DeserializeOwned;
#[cfg(test)]
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use super::error::{read_oidc_json, OidcDependencyError, OidcDependencyFailure, OidcOperation};

/// Trait every discovery-doc shape must implement so the provider can read
/// the two universally-required fields. Extended fields (e.g. `token_endpoint`)
/// stay private to the calling crate.
pub trait DiscoveryDoc: DeserializeOwned + Send + Sync + 'static {
    fn issuer(&self) -> &str;
    fn jwks_uri(&self) -> &str;
}

/// Caller-supplied post-fetch validator for enforcing config-conditional
/// required endpoints (e.g. `token_endpoint` when `verification_mode != JwksOnly`).
/// Default is no-op.
pub type DiscoveryValidator<D> = Arc<dyn Fn(&D) -> Result<(), OidcDependencyFailure> + Send + Sync>;

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/45_function_item.rs");

#[derive(Debug, Clone)]
pub struct JwksProviderConfig {
    pub discovery_url: String,
    pub jwks_cache_ttl: Duration,
    pub jwks_min_refresh_interval: Duration,
    pub discovery_max_response_bytes: usize,
    pub jwks_max_response_bytes: usize,
    pub request_timeout: Duration,
    /// How long a failed discovery outcome is reused before the next fetch.
    /// Production uses [`DISCOVERY_FAILURE_BACKOFF`]; tests shorten it.
    pub discovery_failure_backoff: Duration,
    /// Stable, low-cardinality component label used as the `plugin` label of
    /// `edgion_gateway_outbound_tls_san_mismatch_total`. Avoid per-instance
    /// dynamic strings.
    pub component_label: &'static str,
}

/// Kong-style short rediscovery gate after a failed discovery fetch.
/// Not an operator field; the next call after this deadline retries.
pub(crate) const DISCOVERY_FAILURE_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct DiscoveryFailure {
    until: Instant,
    error: OidcDependencyError,
}

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/78_struct_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/85_impl_item.rs");

#[derive(Debug, Default)]
struct JwksState {
    set: Option<Arc<JwkSet>>,
    expires_at: Option<Instant>,
    last_refresh_at: Option<Instant>,
}

/// Result of a JWKS lookup, carrying a staleness signal alongside the set.
///
/// `get_jwks` transparently serves a cached set when the upstream fetch fails
/// (stale-while-revalidate). Without this flag the caller cannot distinguish
/// that degraded serve from a normal fresh/cached hit, so it cannot record a
/// `fail-open` dye outcome for it.
#[derive(Debug, Clone)]
pub struct JwksLookup {
    pub set: Arc<JwkSet>,
    /// True only when the most recent fetch attempt failed and a previously
    /// cached set was served in its place. False for every other return path
    /// (TTL fast-path hit, min-refresh-blocked fast-path hit, or a fresh
    /// successful fetch) — those are normal cache behavior, not degradation.
    pub served_stale: bool,
}

impl JwksState {
    fn publish_fetch_result(
        &mut self,
        fetch_result: Result<JwkSet, OidcDependencyError>,
        completed_at: Instant,
        ttl: Duration,
        min_refresh: Duration,
    ) -> Result<JwksLookup, OidcDependencyError> {
        self.last_refresh_at = Some(completed_at);

        match fetch_result {
            Ok(jwks) => {
                let jwks = Arc::new(jwks);
                self.set = Some(jwks.clone());
                self.expires_at = Some(completed_at + ttl);
                Ok(JwksLookup {
                    set: jwks,
                    served_stale: false,
                })
            }
            Err(err) => match self.set.clone() {
                Some(cached) => {
                    // Stale-while-revalidate: serve stale and let the caller record the
                    // degradation (`discovery_jwks.rs` maps `served_stale` to a fail-open
                    // dye outcome in the access log).
                    // Bump expires_at by min_refresh_interval so subsequent calls within the
                    // backoff window hit the fast path instead of re-fetching every request
                    // during a prolonged IdP outage.
                    self.expires_at = Some(completed_at + min_refresh);
                    Ok(JwksLookup {
                        set: cached,
                        served_stale: true,
                    })
                }
                None => {
                    self.expires_at = None;
                    Err(err)
                }
            },
        }
    }
}

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/164_struct_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/169_const_item.rs");
#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/171_const_item.rs");
#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/173_const_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/176_enum_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/184_struct_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/191_struct_item.rs");

#[cfg(test)]
include!("../../../../tests/support/plugins/http/openid_connect/jwks_provider_test/200_impl_item.rs");

pub struct JwksProvider<D: DiscoveryDoc> {
    config: JwksProviderConfig,
    client: Arc<Client>,
    validator: DiscoveryValidator<D>,
    discovery_doc: RwLock<Option<Arc<D>>>,
    discovery_refresh: Mutex<()>,
    discovery_failure: RwLock<Option<DiscoveryFailure>>,
    jwks_state: RwLock<JwksState>,
    jwks_refresh: Mutex<()>,
    #[cfg(test)]
    scripted_http: ScriptMutex<Option<Arc<ScriptedHttp>>>,
}

impl<D: DiscoveryDoc> JwksProvider<D> {
    /// Construct. The HTTP client is injected so callers can choose between
    /// a per-instance `Client` (admin) or a process-wide pool (gateway plugin).
    pub fn new(config: JwksProviderConfig, client: Arc<Client>, validator: DiscoveryValidator<D>) -> Arc<Self> {
        Arc::new(Self {
            config,
            client,
            validator,
            discovery_doc: RwLock::new(None),
            discovery_refresh: Mutex::new(()),
            discovery_failure: RwLock::new(None),
            jwks_state: RwLock::new(JwksState::default()),
            jwks_refresh: Mutex::new(()),
            #[cfg(test)]
            scripted_http: ScriptMutex::new(None),
        })
    }

    async fn outbound_get(
        &self,
        url: &str,
        operation: OidcOperation,
    ) -> Result<reqwest::Response, OidcDependencyError> {
        #[cfg(test)]
        {
            let script = self.scripted_http.lock().clone();
            if let Some(script) = script {
                return script.exchange(url, operation);
            }
        }
        self.client
            .get(url)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|error| OidcDependencyError::transport(operation, &error, self.config.component_label))
    }

    /// Read-only cached discovery (no fetch). Used by gateway logout path which
    /// must not pay an HTTP round-trip on the request.
    pub async fn cached_discovery(&self) -> Option<Arc<D>> {
        self.discovery_doc.read().await.clone()
    }

    /// Get cached discovery, fetch if absent. Validator runs **before** caching;
    /// a failed validator does NOT poison the cache. Transport, HTTP, parse, and
    /// validator failures share a short backoff so concurrent cold callers do
    /// not turn one provider outage into one timeout per waiter.
    pub async fn get_or_fetch_discovery(&self) -> Result<Arc<D>, OidcDependencyError> {
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }
        if let Some(error) = self.discovery_backoff_error().await {
            return Err(error);
        }
        let _guard = self.discovery_refresh.lock().await;
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }
        if let Some(error) = self.discovery_backoff_error().await {
            return Err(error);
        }

        match self.fetch_and_validate_discovery().await {
            Ok(doc) => {
                *self.discovery_failure.write().await = None;
                *self.discovery_doc.write().await = Some(doc.clone());
                Ok(doc)
            }
            Err(error) => Err(self.record_discovery_failure(error).await),
        }
    }

    async fn discovery_backoff_error(&self) -> Option<OidcDependencyError> {
        let failure = self.discovery_failure.read().await;
        failure.as_ref().and_then(|failure| {
            if Instant::now() < failure.until {
                Some(failure.error)
            } else {
                None
            }
        })
    }

    async fn record_discovery_failure(&self, error: OidcDependencyError) -> OidcDependencyError {
        *self.discovery_failure.write().await = Some(DiscoveryFailure {
            until: Instant::now() + self.config.discovery_failure_backoff,
            error,
        });
        error
    }

    async fn fetch_and_validate_discovery(&self) -> Result<Arc<D>, OidcDependencyError> {
        super::plugin::admit_oidc_outbound_url(&self.config.discovery_url)
            .map_err(|failure| OidcDependencyError::new(OidcOperation::Discovery, failure))?;

        let resp = self
            .outbound_get(&self.config.discovery_url, OidcOperation::Discovery)
            .await?;
        if !resp.status().is_success() {
            return Err(OidcDependencyError::new(
                OidcOperation::Discovery,
                OidcDependencyFailure::HttpStatus,
            ));
        }
        let doc: D = read_oidc_json(resp, self.config.discovery_max_response_bytes)
            .await
            .map_err(|failure| OidcDependencyError::new(OidcOperation::Discovery, failure))?;

        if doc.issuer().is_empty() {
            return Err(OidcDependencyError::new(
                OidcOperation::Discovery,
                OidcDependencyFailure::InvalidDocument,
            ));
        }
        if doc.jwks_uri().is_empty() {
            return Err(OidcDependencyError::new(
                OidcOperation::Discovery,
                OidcDependencyFailure::InvalidDocument,
            ));
        }
        // Caller-supplied config-aware validation runs before caching.
        (self.validator)(&doc).map_err(|failure| OidcDependencyError::new(OidcOperation::Discovery, failure))?;
        Ok(Arc::new(doc))
    }

    /// Get JWKS. Stale-while-revalidate: if the upstream fetch fails and we
    /// have a cached set, return the cached set flagged `served_stale`. Only
    /// when **no** cached set exists does the error propagate.
    ///
    /// `force_refresh=true` skips the TTL fast-path (used on JWT `kid` miss);
    /// `jwks_min_refresh_interval` still gates back-to-back refreshes.
    pub async fn get_jwks(&self, force_refresh: bool) -> Result<JwksLookup, OidcDependencyError> {
        let now = Instant::now();
        let min_refresh = self.config.jwks_min_refresh_interval;
        let ttl = self.config.jwks_cache_ttl;

        // Fast path.
        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if !force_refresh && state.expires_at.is_some_and(|exp| now < exp) {
                    return Ok(JwksLookup {
                        set: set.clone(),
                        served_stale: false,
                    });
                }
                if force_refresh && !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                    return Ok(JwksLookup {
                        set: set.clone(),
                        served_stale: false,
                    });
                }
            }
        }

        let _guard = self.jwks_refresh.lock().await;
        let now = Instant::now();

        // Double-check after lock.
        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if !force_refresh && state.expires_at.is_some_and(|exp| now < exp) {
                    return Ok(JwksLookup {
                        set: set.clone(),
                        served_stale: false,
                    });
                }
                if force_refresh && !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                    return Ok(JwksLookup {
                        set: set.clone(),
                        served_stale: false,
                    });
                }
            } else if !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                // No cached set AND blocked by min_refresh: caller should retry later.
                return Err(OidcDependencyError::new(
                    OidcOperation::Jwks,
                    OidcDependencyFailure::RefreshThrottled,
                ));
            }
        }

        let discovery = self.get_or_fetch_discovery().await?;
        let fetch_result: Result<JwkSet, OidcDependencyError> = async {
            super::plugin::admit_oidc_outbound_url(discovery.jwks_uri())
                .map_err(|failure| OidcDependencyError::new(OidcOperation::Jwks, failure))?;
            let resp = self.outbound_get(discovery.jwks_uri(), OidcOperation::Jwks).await?;
            if !resp.status().is_success() {
                return Err(OidcDependencyError::new(
                    OidcOperation::Jwks,
                    OidcDependencyFailure::HttpStatus,
                ));
            }
            let jwks: JwkSet = read_oidc_json(resp, self.config.jwks_max_response_bytes)
                .await
                .map_err(|failure| OidcDependencyError::new(OidcOperation::Jwks, failure))?;
            if jwks.keys.is_empty() {
                return Err(OidcDependencyError::new(
                    OidcOperation::Jwks,
                    OidcDependencyFailure::InvalidDocument,
                ));
            }
            Ok(jwks)
        }
        .await;

        let mut state = self.jwks_state.write().await;
        let completed_at = Instant::now();
        state.publish_fetch_result(fetch_result, completed_at, ttl, min_refresh)
    }

    fn can_refresh(last: Option<Instant>, now: Instant, min: Duration) -> bool {
        match last {
            Some(t) => now.saturating_duration_since(t) >= min,
            None => true,
        }
    }

    /// Test-only helper to inject a discovery doc directly, bypassing fetch.
    /// Used by OIDC plugin tests that exercise post-discovery code paths.
    #[cfg(test)]
    pub async fn seed_discovery(&self, doc: D) {
        *self.discovery_failure.write().await = None;
        *self.discovery_doc.write().await = Some(Arc::new(doc));
    }

    /// Seed an expired set so callers can exercise stale fallback without network I/O.
    #[cfg(test)]
    pub async fn seed_stale_jwks(&self, set: JwkSet) {
        *self.jwks_state.write().await = JwksState {
            set: Some(Arc::new(set)),
            expires_at: Some(Instant::now()),
            last_refresh_at: None,
        };
    }

    /// Install an in-memory HTTP script. Unit tests must not dial.
    #[cfg(test)]
    pub fn install_scripted_http(&self, script: Arc<ScriptedHttp>) {
        *self.scripted_http.lock() = Some(script);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/openid_connect/jwks_provider_test.rs"]
mod tests;

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
pub fn no_op_validator<D: DiscoveryDoc>() -> DiscoveryValidator<D> {
    Arc::new(|_| Ok(()))
}

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

/// Minimal discovery doc test fixture — only the two universal fields.
#[cfg(test)]
#[derive(Debug, Deserialize)]
pub struct LeanDiscoveryDocument {
    pub issuer: String,
    pub jwks_uri: String,
}

#[cfg(test)]
impl DiscoveryDoc for LeanDiscoveryDocument {
    fn issuer(&self) -> &str {
        &self.issuer
    }
    fn jwks_uri(&self) -> &str {
        &self.jwks_uri
    }
}

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

/// In-memory IdP HTTP script for unit tests. Compiled only under `cfg(test)`;
/// production builds keep the canonical reqwest client with no extra field or
/// branch on the request path.
#[cfg(test)]
pub struct ScriptedHttp {
    inner: ScriptMutex<std::collections::HashMap<String, ScriptedRoute>>,
}

#[cfg(test)]
pub const TEST_IDP_ORIGIN: &str = "http://oidc.test";
#[cfg(test)]
pub const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";
#[cfg(test)]
pub const JWKS_PATH: &str = "/jwks";

#[cfg(test)]
#[derive(Clone)]
enum ScriptedEvent {
    Http { status: u16, body: Vec<u8> },
    Timeout,
    Transport,
}

#[cfg(test)]
struct LiveJwks {
    fail: Arc<std::sync::atomic::AtomicBool>,
    keys: Arc<std::sync::atomic::AtomicUsize>,
    hits: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
#[derive(Default)]
struct ScriptedRoute {
    queue: std::collections::VecDeque<ScriptedEvent>,
    repeat: Option<ScriptedEvent>,
    live_jwks: Option<LiveJwks>,
    hits: usize,
}

#[cfg(test)]
impl ScriptedHttp {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: ScriptMutex::new(std::collections::HashMap::new()),
        })
    }

    pub fn origin() -> &'static str {
        TEST_IDP_ORIGIN
    }

    pub fn jwks_url() -> String {
        format!("{TEST_IDP_ORIGIN}{JWKS_PATH}")
    }

    pub fn enqueue_http(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.push(
            path,
            ScriptedEvent::Http {
                status,
                body: body.into(),
            },
            false,
        );
    }

    pub fn enqueue_http_repeat(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.push(
            path,
            ScriptedEvent::Http {
                status,
                body: body.into(),
            },
            true,
        );
    }

    pub fn enqueue_timeout(&self, path: &str) {
        self.push(path, ScriptedEvent::Timeout, false);
    }

    pub fn enqueue_transport(&self, path: &str) {
        self.push(path, ScriptedEvent::Transport, false);
    }

    pub fn mount_ok_discovery(&self) {
        let body = serde_json::json!({
            "issuer": TEST_IDP_ORIGIN,
            "jwks_uri": Self::jwks_url(),
        })
        .to_string()
        .into_bytes();
        self.enqueue_http_repeat(DISCOVERY_PATH, 200, body);
    }

    pub fn mount_live_jwks(
        &self,
        fail: Arc<std::sync::atomic::AtomicBool>,
        keys: Arc<std::sync::atomic::AtomicUsize>,
        hits: Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let mut routes = self.inner.lock();
        let route = routes.entry(JWKS_PATH.to_string()).or_default();
        route.live_jwks = Some(LiveJwks { fail, keys, hits });
    }

    pub fn hits(&self, path: &str) -> usize {
        self.inner.lock().get(path).map(|route| route.hits).unwrap_or(0)
    }

    fn push(&self, path: &str, event: ScriptedEvent, repeat: bool) {
        let mut routes = self.inner.lock();
        let route = routes.entry(path.to_string()).or_default();
        if repeat {
            route.repeat = Some(event);
        } else {
            route.queue.push_back(event);
        }
    }

    fn route_key(url: &str) -> &'static str {
        if url.contains(DISCOVERY_PATH) {
            DISCOVERY_PATH
        } else if url.contains(JWKS_PATH) {
            JWKS_PATH
        } else {
            panic!("OIDC unit test scripted HTTP has no route for {url}");
        }
    }

    fn exchange(&self, url: &str, operation: OidcOperation) -> Result<reqwest::Response, OidcDependencyError> {
        let path = Self::route_key(url);
        let event = {
            let mut routes = self.inner.lock();
            let route = routes.entry(path.to_string()).or_default();
            route.hits += 1;
            if let Some(live) = &route.live_jwks {
                live.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if live.fail.load(std::sync::atomic::Ordering::SeqCst) {
                    ScriptedEvent::Http {
                        status: 503,
                        body: Vec::new(),
                    }
                } else {
                    let n = live.keys.load(std::sync::atomic::Ordering::SeqCst);
                    let keys = (0..n)
                        .map(|i| format!(r#"{{"kty":"oct","kid":"k{i}","k":"AAAAAA","alg":"HS256"}}"#))
                        .collect::<Vec<_>>()
                        .join(",");
                    ScriptedEvent::Http {
                        status: 200,
                        body: format!(r#"{{"keys":[{keys}]}}"#).into_bytes(),
                    }
                }
            } else if let Some(event) = route.queue.pop_front() {
                event
            } else if let Some(event) = route.repeat.clone() {
                event
            } else {
                panic!("OIDC unit test scripted HTTP exhausted for {path} ({url})");
            }
        };
        match event {
            ScriptedEvent::Http { status, body } => Ok(http::Response::builder()
                .status(status)
                .body(body)
                .unwrap_or_else(|_| panic!("scripted OIDC HTTP status {status} must be valid"))
                .into()),
            ScriptedEvent::Timeout => Err(OidcDependencyError::new(operation, OidcDependencyFailure::Timeout)),
            ScriptedEvent::Transport => Err(OidcDependencyError::new(operation, OidcDependencyFailure::Transport)),
        }
    }
}

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
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovery_backoff_replays_static_san_failure_without_counting_again() {
        use crate::observe::metrics::registry::names;
        use metrics_util::debugging::DebuggingRecorder;

        let mut config = wm_config("https://private-provider.example", 60_000, 0, 2_000);
        config.discovery_failure_backoff = Duration::from_secs(5);
        let provider = build_provider(config);
        let failure = OidcDependencyError::new(OidcOperation::Discovery, OidcDependencyFailure::TlsSanMismatch);
        provider.record_discovery_failure(failure).await;

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        {
            let _guard = metrics::set_default_local_recorder(&recorder);
            for _ in 0..3 {
                let replay = provider.get_or_fetch_discovery().await.unwrap_err();
                assert_eq!(replay, failure);
                assert_eq!(replay.to_string(), "discovery san mismatch");
                assert!(!format!("{replay:?}").contains("private-provider"));
            }
        }
        assert!(snapshotter
            .snapshot()
            .into_vec()
            .iter()
            .all(|(key, _, _, _)| key.key().name() != names::OUTBOUND_TLS_SAN_MISMATCH_TOTAL));
    }

    #[test]
    fn jwks_publication_deadlines_start_at_completion() {
        let started_at = Instant::now();
        let completed_at = started_at + Duration::from_secs(10);
        let ttl = Duration::from_secs(30);
        let min_refresh = Duration::from_secs(5);

        let mut successful = JwksState::default();
        let fresh = successful
            .publish_fetch_result(Ok(JwkSet { keys: Vec::new() }), completed_at, ttl, min_refresh)
            .expect("successful fetch must publish the new set");
        assert_eq!(successful.last_refresh_at, Some(completed_at));
        assert_eq!(successful.expires_at, Some(completed_at + ttl));
        assert!(!fresh.served_stale);
        assert!(Arc::ptr_eq(
            successful.set.as_ref().expect("successful fetch must cache the set"),
            &fresh.set
        ));

        let cached = fresh.set.clone();
        let mut stale = JwksState {
            set: Some(cached.clone()),
            expires_at: Some(started_at),
            last_refresh_at: Some(started_at),
        };
        let fallback = stale
            .publish_fetch_result(
                Err(OidcDependencyError::new(
                    OidcOperation::Jwks,
                    OidcDependencyFailure::Transport,
                )),
                completed_at,
                ttl,
                min_refresh,
            )
            .expect("cached failure must serve the stale set");
        assert_eq!(stale.last_refresh_at, Some(completed_at));
        assert_eq!(stale.expires_at, Some(completed_at + min_refresh));
        assert!(fallback.served_stale);
        assert!(Arc::ptr_eq(&cached, &fallback.set));
        assert!(Arc::ptr_eq(
            stale.set.as_ref().expect("stale fallback must retain the cached set"),
            &fallback.set
        ));
    }

    #[test]
    fn provider_error_display_is_static() {
        assert_eq!(
            OidcDependencyError::new(OidcOperation::Discovery, OidcDependencyFailure::Transport).to_string(),
            "discovery transport fail"
        );
        assert_eq!(
            OidcDependencyError::new(OidcOperation::Discovery, OidcDependencyFailure::TlsSanMismatch).to_string(),
            "discovery san mismatch"
        );
    }

    #[test]
    fn test_lean_discovery_document_deserialize() {
        let json = r#"{
            "issuer": "https://idp.example.com",
            "jwks_uri": "https://idp.example.com/certs",
            "authorization_endpoint": "https://idp.example.com/auth"
        }"#;
        let doc: LeanDiscoveryDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.issuer(), "https://idp.example.com");
        assert_eq!(doc.jwks_uri(), "https://idp.example.com/certs");
    }

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct ScriptedIdp {
        origin: &'static str,
        script: Arc<ScriptedHttp>,
        jwks_should_fail: Arc<AtomicBool>,
        jwks_keys_count: Arc<AtomicUsize>,
        jwks_request_count: Arc<AtomicUsize>,
    }

    fn scripted_idp() -> ScriptedIdp {
        let jwks_should_fail = Arc::new(AtomicBool::new(false));
        let jwks_keys_count = Arc::new(AtomicUsize::new(1));
        let jwks_request_count = Arc::new(AtomicUsize::new(0));
        let script = ScriptedHttp::new();
        script.mount_ok_discovery();
        script.mount_live_jwks(
            jwks_should_fail.clone(),
            jwks_keys_count.clone(),
            jwks_request_count.clone(),
        );
        ScriptedIdp {
            origin: ScriptedHttp::origin(),
            script,
            jwks_should_fail,
            jwks_keys_count,
            jwks_request_count,
        }
    }

    fn unused_client() -> Arc<Client> {
        Arc::new(Client::new())
    }

    fn test_config(origin: &str, ttl_ms: u64, min_refresh_ms: u64) -> JwksProviderConfig {
        wm_config(origin, ttl_ms, min_refresh_ms, 2_000)
    }

    fn test_provider(idp: &ScriptedIdp, ttl_ms: u64, min_refresh_ms: u64) -> Arc<JwksProvider<LeanDiscoveryDocument>> {
        let provider = JwksProvider::new(
            test_config(idp.origin, ttl_ms, min_refresh_ms),
            unused_client(),
            no_op_validator(),
        );
        provider.install_scripted_http(idp.script.clone());
        provider
    }

    fn wm_config(origin: &str, ttl_ms: u64, min_refresh_ms: u64, timeout_ms: u64) -> JwksProviderConfig {
        JwksProviderConfig {
            discovery_url: format!("{origin}{DISCOVERY_PATH}"),
            jwks_cache_ttl: Duration::from_millis(ttl_ms),
            jwks_min_refresh_interval: Duration::from_millis(min_refresh_ms),
            discovery_max_response_bytes: 65_536,
            jwks_max_response_bytes: 1_048_576,
            request_timeout: Duration::from_millis(timeout_ms),
            discovery_failure_backoff: Duration::from_millis(200),
            component_label: "test",
        }
    }

    fn build_provider(cfg: JwksProviderConfig) -> Arc<JwksProvider<LeanDiscoveryDocument>> {
        JwksProvider::new(cfg, unused_client(), no_op_validator())
    }

    fn build_scripted(cfg: JwksProviderConfig, script: Arc<ScriptedHttp>) -> Arc<JwksProvider<LeanDiscoveryDocument>> {
        let provider = build_provider(cfg);
        provider.install_scripted_http(script);
        provider
    }

    fn oct_jwks(kids: &[(&str, &str)]) -> Vec<u8> {
        let keys: Vec<serde_json::Value> = kids
            .iter()
            .map(|(kid, k)| serde_json::json!({"kty":"oct","kid": kid, "k": k, "alg":"HS256"}))
            .collect();
        serde_json::json!({"keys": keys}).to_string().into_bytes()
    }

    #[tokio::test]
    async fn discovery_cache_returns_one_immutable_snapshot() {
        let idp = scripted_idp();
        let p = test_provider(&idp, 60_000, 100);

        let fetched = p.get_or_fetch_discovery().await.unwrap();
        let cached_fetch = p.get_or_fetch_discovery().await.unwrap();
        let cached_read = p.cached_discovery().await.unwrap();
        assert!(Arc::ptr_eq(&fetched, &cached_fetch));
        assert!(Arc::ptr_eq(&fetched, &cached_read));
        assert_eq!(fetched.issuer(), idp.origin);

        let seeded = test_provider(&idp, 60_000, 100);
        seeded
            .seed_discovery(LeanDiscoveryDocument {
                issuer: "https://seeded.example.com".to_string(),
                jwks_uri: "https://seeded.example.com/jwks".to_string(),
            })
            .await;
        let seeded_fetch = seeded.get_or_fetch_discovery().await.unwrap();
        let seeded_read = seeded.cached_discovery().await.unwrap();
        assert!(Arc::ptr_eq(&seeded_fetch, &seeded_read));
        assert_eq!(seeded_fetch.issuer(), "https://seeded.example.com");
    }

    #[tokio::test]
    async fn fast_path_returns_cached_within_ttl() {
        let idp = scripted_idp();
        let p = test_provider(&idp, 60_000, 100);
        let a = p.get_jwks(false).await.unwrap();
        let b = p.get_jwks(false).await.unwrap();
        assert_eq!(a.set.keys.len(), b.set.keys.len());
        assert!(Arc::ptr_eq(&a.set, &b.set));
        assert!(!a.served_stale, "TTL fast-path hit must not be flagged stale");
        assert!(!b.served_stale, "TTL fast-path hit must not be flagged stale");
    }

    /// Task 11: `get_jwks` must surface a staleness signal distinguishing a
    /// transparent stale-cache serve (fetch failed, cached set returned)
    /// from a normal fresh/cached hit — the caller uses this to record the
    /// `fail-open:jwks-stale-cache` dye outcome.
    #[tokio::test]
    async fn stale_fallback_returns_cached_on_fetch_error() {
        let idp = scripted_idp();
        let p = test_provider(&idp, 50, 0);
        let fresh = p.get_jwks(false).await.unwrap();
        assert!(!fresh.served_stale, "initial fresh fetch must not be flagged stale");
        idp.jwks_should_fail.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let stale = p.get_jwks(false).await.expect("stale fallback should succeed");
        assert_eq!(stale.set.keys.len(), 1);
        assert!(
            stale.served_stale,
            "cache served after a failed fetch must be flagged served_stale"
        );
        assert!(Arc::ptr_eq(&fresh.set, &stale.set));
    }

    #[tokio::test]
    async fn no_cache_no_fallback_returns_error() {
        let idp = scripted_idp();
        idp.jwks_should_fail.store(true, Ordering::SeqCst);
        let p = test_provider(&idp, 60_000, 0);
        let err = p
            .get_jwks(false)
            .await
            .expect_err("must error with empty cache")
            .to_string();
        assert!(err.contains("503") || err.contains("status"), "got: {}", err);
    }

    #[tokio::test]
    async fn force_refresh_blocked_by_min_interval_returns_cached() {
        let idp = scripted_idp();
        let p = test_provider(&idp, 60_000, 60_000);
        let initial = p.get_jwks(false).await.unwrap();
        idp.jwks_keys_count.store(5, Ordering::SeqCst);
        let still_old = p.get_jwks(true).await.unwrap();
        assert_eq!(still_old.set.keys.len(), 1);
        assert!(Arc::ptr_eq(&initial.set, &still_old.set));
        assert!(
            !still_old.served_stale,
            "min-refresh-blocked fast path is not a fetch failure, must not be flagged stale"
        );
    }

    #[tokio::test]
    async fn singleflight_concurrent_callers_share_one_fetch() {
        let idp = scripted_idp();
        let p = test_provider(&idp, 60_000, 0);
        let mut handles = vec![];
        for _ in 0..20 {
            let pc = p.clone();
            handles.push(tokio::spawn(async move { pc.get_jwks(false).await }));
        }
        for h in handles {
            let _ = h.await.unwrap().unwrap();
        }
        assert_eq!(
            idp.jwks_request_count.load(Ordering::SeqCst),
            1,
            "20 concurrent get_jwks should result in exactly 1 upstream JWKS fetch (singleflight)"
        );
    }

    #[tokio::test]
    async fn stale_fallback_rate_limited_by_min_refresh() {
        // During a prolonged outage with a cached set, we must not hammer the JWKS
        // endpoint on every request. Subsequent calls within min_refresh_interval after
        // a failed refresh should hit the fast path.
        let idp = scripted_idp();
        let p = test_provider(&idp, 30, 5_000); // 30ms TTL, 5s min_refresh
        let fresh = p.get_jwks(false).await.unwrap(); // populate cache (1 request)
        idp.jwks_should_fail.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await; // expire TTL
        let stale = p.get_jwks(false).await.unwrap(); // stale-fallback (1 more request, fails)
        assert!(stale.served_stale);
        assert!(Arc::ptr_eq(&fresh.set, &stale.set));
        let after_first_stale = idp.jwks_request_count.load(Ordering::SeqCst);
        for _ in 0..10 {
            let backoff_hit = p.get_jwks(false).await.unwrap();
            assert!(!backoff_hit.served_stale);
            assert!(Arc::ptr_eq(&fresh.set, &backoff_hit.set));
        }
        let after_burst = idp.jwks_request_count.load(Ordering::SeqCst);
        assert_eq!(
            after_burst,
            after_first_stale,
            "stale-fallback should bump expires_at to absorb subsequent calls within min_refresh window; \
             saw {} extra requests during burst (expected 0)",
            after_burst - after_first_stale
        );
    }

    #[tokio::test]
    async fn validator_rejection_does_not_cache() {
        let idp = scripted_idp();
        let cfg = test_config(idp.origin, 60_000, 0);
        let validator: DiscoveryValidator<LeanDiscoveryDocument> =
            Arc::new(|_| Err(OidcDependencyFailure::InvalidDocument));
        let p = JwksProvider::new(cfg, unused_client(), validator);
        p.install_scripted_http(idp.script.clone());
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("validator should reject")
            .to_string();
        assert!(err == "discovery document invalid", "got: {}", err);
        assert!(
            p.cached_discovery().await.is_none(),
            "cache must remain empty after validator failure"
        );
    }

    #[tokio::test]
    async fn test_jwks_fetch_timeout_returns_error_when_no_cache() {
        let script = ScriptedHttp::new();
        script.mount_ok_discovery();
        script.enqueue_timeout(JWKS_PATH);
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 100), script);
        let err = p
            .get_jwks(false)
            .await
            .expect_err("timeout must error with empty cache")
            .to_string();
        assert_eq!(err, "jwks timeout");
    }

    #[tokio::test]
    async fn test_jwks_fetch_5xx_returns_error_when_no_cache() {
        let script = ScriptedHttp::new();
        script.mount_ok_discovery();
        script.enqueue_http_repeat(JWKS_PATH, 500, Vec::new());
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let err = p
            .get_jwks(false)
            .await
            .expect_err("5xx without cache must error")
            .to_string();
        assert!(
            err.contains("500") || err.contains("status"),
            "expected status error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_jwks_fetch_5xx_falls_back_to_cache() {
        let script = ScriptedHttp::new();
        script.mount_ok_discovery();
        script.enqueue_http(JWKS_PATH, 200, oct_jwks(&[("k0", "AAAAAA")]));
        script.enqueue_http_repeat(JWKS_PATH, 503, Vec::new());
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 30, 0, 2_000), script);
        let first = p.get_jwks(false).await.expect("initial fetch ok");
        assert_eq!(first.set.keys.len(), 1);
        assert!(!first.served_stale);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let second = p.get_jwks(false).await.expect("stale fallback must serve cache on 5xx");
        assert_eq!(second.set.keys.len(), 1, "should serve cached set after 5xx");
        assert!(
            second.served_stale,
            "5xx fallback to cache must be flagged served_stale"
        );
    }

    #[tokio::test]
    async fn test_jwks_malformed_json_returns_parse_error() {
        let script = ScriptedHttp::new();
        script.mount_ok_discovery();
        script.enqueue_http_repeat(JWKS_PATH, 200, b"{not-valid-json".to_vec());
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let err = p
            .get_jwks(false)
            .await
            .expect_err("malformed JSON must error")
            .to_string();
        let lc = err.to_lowercase();
        assert!(
            lc.contains("parse") || lc.contains("jwks"),
            "expected parse error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_jwks_empty_keys_returns_error() {
        let script = ScriptedHttp::new();
        script.mount_ok_discovery();
        script.enqueue_http_repeat(JWKS_PATH, 200, serde_json::json!({"keys": []}).to_string().into_bytes());
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let err = p.get_jwks(false).await.expect_err("empty keys must error").to_string();
        assert!(
            err == "jwks document invalid",
            "expected 'empty keys' message, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_discovery_5xx_returns_error() {
        let script = ScriptedHttp::new();
        script.enqueue_http_repeat(DISCOVERY_PATH, 503, Vec::new());
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("discovery 5xx must error")
            .to_string();
        assert!(
            err.contains("503") || err.contains("status"),
            "expected discovery status error, got: {err}"
        );
        assert!(
            p.cached_discovery().await.is_none(),
            "discovery cache must remain empty after upstream 5xx"
        );
    }

    #[tokio::test]
    async fn test_discovery_failure_backoff_reuses_error_without_refetch() {
        let script = ScriptedHttp::new();
        script.enqueue_http(DISCOVERY_PATH, 503, Vec::new());
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script.clone());
        let first = p.get_or_fetch_discovery().await.expect_err("discovery 5xx must error");
        let second = p
            .get_or_fetch_discovery()
            .await
            .expect_err("backoff must reuse the failed outcome");
        assert_eq!(first, second);
        assert_eq!(script.hits(DISCOVERY_PATH), 1);
        assert!(
            p.cached_discovery().await.is_none(),
            "failure backoff must not publish a discovery snapshot"
        );
    }

    #[tokio::test]
    async fn test_discovery_recovers_after_failure_backoff() {
        let script = ScriptedHttp::new();
        script.enqueue_http(DISCOVERY_PATH, 503, Vec::new());
        script.mount_ok_discovery();
        let mut cfg = wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000);
        cfg.discovery_failure_backoff = Duration::from_millis(50);
        let p = build_scripted(cfg, script);
        p.get_or_fetch_discovery()
            .await
            .expect_err("first discovery attempt must fail");
        tokio::time::sleep(Duration::from_millis(80)).await;
        let recovered = p
            .get_or_fetch_discovery()
            .await
            .expect("retry after backoff must succeed");
        let cached = p.cached_discovery().await.expect("recovery publishes a snapshot");
        assert!(Arc::ptr_eq(&recovered, &cached));
    }

    #[tokio::test]
    async fn test_discovery_concurrent_failures_one_fetch_then_backoff() {
        let script = ScriptedHttp::new();
        script.enqueue_http(DISCOVERY_PATH, 503, Vec::new());
        let mut cfg = wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000);
        cfg.discovery_failure_backoff = Duration::from_secs(2);
        let p = build_scripted(cfg, script.clone());
        let mut handles = Vec::new();
        for _ in 0..16 {
            let provider = p.clone();
            handles.push(tokio::spawn(async move { provider.get_or_fetch_discovery().await }));
        }
        for handle in handles {
            handle
                .await
                .expect("join")
                .expect_err("concurrent cold callers must see the failed fetch");
        }
        for _ in 0..8 {
            p.get_or_fetch_discovery()
                .await
                .expect_err("callers inside the backoff window must not contact the provider");
        }
        assert_eq!(script.hits(DISCOVERY_PATH), 1);
    }

    #[tokio::test]
    async fn test_discovery_malformed_json_returns_error() {
        let script = ScriptedHttp::new();
        script.enqueue_http_repeat(DISCOVERY_PATH, 200, b"not-json".to_vec());
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("discovery malformed JSON must error")
            .to_string();
        assert_eq!(err, "discovery response json fail");
    }

    #[tokio::test]
    async fn test_discovery_missing_jwks_uri_returns_error() {
        let script = ScriptedHttp::new();
        let body = serde_json::json!({
            "issuer": ScriptedHttp::origin(),
            "jwks_uri": "",
        })
        .to_string()
        .into_bytes();
        script.enqueue_http_repeat(DISCOVERY_PATH, 200, body);
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("missing jwks_uri must error")
            .to_string();
        assert!(
            err == "discovery document invalid",
            "expected 'jwks_uri' in error message, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_discovery_missing_issuer_returns_error() {
        let script = ScriptedHttp::new();
        let body = serde_json::json!({
            "issuer": "",
            "jwks_uri": ScriptedHttp::jwks_url(),
        })
        .to_string()
        .into_bytes();
        script.enqueue_http_repeat(DISCOVERY_PATH, 200, body);
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("missing issuer must error")
            .to_string();
        assert!(
            err == "discovery document invalid",
            "expected 'issuer' in error message, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_discovery_unreachable_returns_error() {
        let script = ScriptedHttp::new();
        script.enqueue_transport(DISCOVERY_PATH);
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 500), script);
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("unreachable discovery must error")
            .to_string();
        assert!(err == "discovery transport fail", "expected fetch failure, got: {err}");
    }

    #[tokio::test]
    async fn test_jwks_kid_rotation_force_refresh_loads_new_keys() {
        let script = ScriptedHttp::new();
        script.mount_ok_discovery();
        script.enqueue_http(JWKS_PATH, 200, oct_jwks(&[("k_old", "AAAAAA")]));
        script.enqueue_http_repeat(JWKS_PATH, 200, oct_jwks(&[("k_new", "BBBBBB")]));
        let p = build_scripted(wm_config(ScriptedHttp::origin(), 60_000, 0, 2_000), script);
        let first = p.get_jwks(false).await.expect("initial fetch ok");
        assert_eq!(first.set.keys.len(), 1);
        let kid_first = first.set.keys[0].common.key_id.as_deref();
        assert_eq!(kid_first, Some("k_old"));

        let rotated = p.get_jwks(true).await.expect("force refresh fetch ok");
        assert_eq!(rotated.set.keys.len(), 1);
        assert!(!Arc::ptr_eq(&first.set, &rotated.set));
        let kid_rot = rotated.set.keys[0].common.key_id.as_deref();
        assert_eq!(kid_rot, Some("k_new"), "force_refresh should pick up rotated kid");
        assert!(
            !rotated
                .set
                .keys
                .iter()
                .any(|k| k.common.key_id.as_deref() == Some("k_old")),
            "old kid must be gone after rotation"
        );
        assert_eq!(first.set.keys[0].common.key_id.as_deref(), Some("k_old"));

        let cached_rotated = p.get_jwks(false).await.expect("rotated set cached");
        assert!(Arc::ptr_eq(&rotated.set, &cached_rotated.set));
    }
}

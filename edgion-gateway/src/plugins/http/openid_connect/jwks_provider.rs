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
use reqwest::Client;
use serde::de::DeserializeOwned;
#[cfg(test)]
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use crate::observe::metrics::registry::record_outbound_tls_san_mismatch;
use crate::plugins::http::common::http_util::fetch_json_limited;
use crate::tls::validation::outbound_san_mismatch_reason;

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
pub type DiscoveryValidator<D> = Arc<dyn Fn(&D) -> Result<(), String> + Send + Sync>;

#[cfg(test)]
pub fn no_op_validator<D: DiscoveryDoc>() -> DiscoveryValidator<D> {
    Arc::new(|_| Ok(()))
}

/// Error type for the discovery / JWKS fetch paths.
///
/// `TlsSanMismatch` is produced **only** by the discovery fetch. The JWKS fetch
/// returns `Other` because `get_jwks` serves a stale cache on error, so the
/// variant would be discarded on exactly the paths that could act on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwksProviderError {
    TlsSanMismatch { reason: String },
    Other(String),
}

impl std::fmt::Display for JwksProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JwksProviderError::TlsSanMismatch { reason } => {
                write!(f, "outbound TLS SAN mismatch: {}", reason)
            }
            JwksProviderError::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for JwksProviderError {}

impl From<String> for JwksProviderError {
    fn from(msg: String) -> Self {
        JwksProviderError::Other(msg)
    }
}

#[derive(Debug, Clone)]
pub struct JwksProviderConfig {
    pub discovery_url: String,
    pub jwks_cache_ttl: Duration,
    pub jwks_min_refresh_interval: Duration,
    pub discovery_max_response_bytes: usize,
    pub jwks_max_response_bytes: usize,
    pub request_timeout: Duration,
    /// Stable, low-cardinality component label used as the `plugin` label of
    /// `edgion_gateway_outbound_tls_san_mismatch_total`. Avoid per-instance
    /// dynamic strings.
    pub component_label: &'static str,
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

pub struct JwksProvider<D: DiscoveryDoc> {
    config: JwksProviderConfig,
    client: Arc<Client>,
    validator: DiscoveryValidator<D>,
    discovery_doc: RwLock<Option<Arc<D>>>,
    discovery_refresh: Mutex<()>,
    jwks_state: RwLock<JwksState>,
    jwks_refresh: Mutex<()>,
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
            jwks_state: RwLock::new(JwksState::default()),
            jwks_refresh: Mutex::new(()),
        })
    }

    /// Read-only cached discovery (no fetch). Used by gateway logout path which
    /// must not pay an HTTP round-trip on the request.
    pub async fn cached_discovery(&self) -> Option<Arc<D>> {
        self.discovery_doc.read().await.clone()
    }

    /// Get cached discovery, fetch if absent. Validator runs **before** caching;
    /// a failed validator does NOT poison the cache (next call retries).
    pub async fn get_or_fetch_discovery(&self) -> Result<Arc<D>, JwksProviderError> {
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }
        let _guard = self.discovery_refresh.lock().await;
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }

        super::plugin::admit_oidc_outbound_url(&self.config.discovery_url)
            .map_err(|error| JwksProviderError::Other(error.to_string()))?;

        let resp = self
            .client
            .get(&self.config.discovery_url)
            .timeout(self.config.request_timeout)
            .send()
            .await
            .map_err(|e| match outbound_san_mismatch_reason(&e) {
                Some(reason) => {
                    record_outbound_tls_san_mismatch(self.config.component_label);
                    JwksProviderError::TlsSanMismatch { reason }
                }
                None => JwksProviderError::Other(format!("Failed to fetch discovery document: {}", e)),
            })?;
        if !resp.status().is_success() {
            return Err(JwksProviderError::Other(format!(
                "Discovery endpoint returned status {}",
                resp.status()
            )));
        }
        let doc: D = fetch_json_limited(resp, self.config.discovery_max_response_bytes, "OIDC discovery")
            .await
            .map_err(JwksProviderError::Other)?;

        if doc.issuer().is_empty() {
            return Err(JwksProviderError::Other(
                "Discovery document missing issuer".to_string(),
            ));
        }
        if doc.jwks_uri().is_empty() {
            return Err(JwksProviderError::Other(
                "Discovery document missing jwks_uri".to_string(),
            ));
        }
        // Caller-supplied config-aware validation runs before caching.
        (self.validator)(&doc).map_err(JwksProviderError::Other)?;

        let doc = Arc::new(doc);
        *self.discovery_doc.write().await = Some(doc.clone());
        Ok(doc)
    }

    /// Get JWKS. Stale-while-revalidate: if the upstream fetch fails and we
    /// have a cached set, return the cached set flagged `served_stale`. Only
    /// when **no** cached set exists does the error propagate.
    ///
    /// `force_refresh=true` skips the TTL fast-path (used on JWT `kid` miss);
    /// `jwks_min_refresh_interval` still gates back-to-back refreshes.
    pub async fn get_jwks(&self, force_refresh: bool) -> Result<JwksLookup, JwksProviderError> {
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
                return Err(JwksProviderError::Other(
                    "JWKS refresh blocked by minimum refresh interval".to_string(),
                ));
            }
        }

        let discovery = self.get_or_fetch_discovery().await?;
        let fetch_result: Result<JwkSet, String> = async {
            super::plugin::admit_oidc_outbound_url(discovery.jwks_uri())?;
            let resp = self
                .client
                .get(discovery.jwks_uri())
                .timeout(self.config.request_timeout)
                .send()
                .await
                .map_err(|e| {
                    if outbound_san_mismatch_reason(&e).is_some() {
                        record_outbound_tls_san_mismatch(self.config.component_label);
                    }
                    format!("JWKS request failed: {}", e)
                })?;
            if !resp.status().is_success() {
                return Err(format!("JWKS request failed with status {}", resp.status()));
            }
            let jwks: JwkSet = fetch_json_limited(resp, self.config.jwks_max_response_bytes, "JWKS").await?;
            if jwks.keys.is_empty() {
                return Err("JWKS endpoint returned empty keys".to_string());
            }
            Ok(jwks)
        }
        .await;

        let mut state = self.jwks_state.write().await;
        state.last_refresh_at = Some(now);

        match fetch_result {
            Ok(jwks) => {
                let jwks = Arc::new(jwks);
                state.set = Some(jwks.clone());
                state.expires_at = Some(now + ttl);
                Ok(JwksLookup {
                    set: jwks,
                    served_stale: false,
                })
            }
            Err(err) => match state.set.clone() {
                Some(cached) => {
                    // Stale-while-revalidate: serve stale and let the caller record the
                    // degradation (`discovery_jwks.rs` maps `served_stale` to a fail-open
                    // dye outcome in the access log).
                    // Bump expires_at by min_refresh_interval so subsequent calls within the
                    // backoff window hit the fast path instead of re-fetching every request
                    // during a prolonged IdP outage.
                    state.expires_at = Some(now + min_refresh);
                    Ok(JwksLookup {
                        set: cached,
                        served_stale: true,
                    })
                }
                None => Err(JwksProviderError::Other(err)),
            },
        }
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
        *self.discovery_doc.write().await = Some(Arc::new(doc));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_display_renders_inner_message() {
        assert_eq!(JwksProviderError::Other("boom".to_string()).to_string(), "boom");
        assert_eq!(
            JwksProviderError::TlsSanMismatch {
                reason: "no match".to_string()
            }
            .to_string(),
            "outbound TLS SAN mismatch: no match"
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct MockIdp {
        addr: String,
        jwks_should_fail: Arc<AtomicBool>,
        jwks_keys_count: Arc<AtomicUsize>,
        jwks_request_count: Arc<AtomicUsize>,
    }

    async fn spawn_mock_idp() -> MockIdp {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let addr = format!("http://{}", local_addr);
        let jwks_should_fail = Arc::new(AtomicBool::new(false));
        let jwks_keys_count = Arc::new(AtomicUsize::new(1));
        let jwks_request_count = Arc::new(AtomicUsize::new(0));
        let fail_clone = jwks_should_fail.clone();
        let count_clone = jwks_keys_count.clone();
        let request_count_clone = jwks_request_count.clone();
        let addr_for_loop = addr.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let fail = fail_clone.clone();
                let count = count_clone.clone();
                let request_count = request_count_clone.clone();
                let addr_owned = addr_for_loop.clone();
                tokio::spawn(async move {
                    let mut req = [0u8; 1024];
                    let n = sock.read(&mut req).await.unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&req[..n]).to_string();
                    // This mock serves exactly one request per connection and then
                    // shuts the socket down, so every response must advertise
                    // `Connection: close`. Without it the HTTP/1.1 default (keep-alive)
                    // would let the reqwest pool reuse a socket the server has already
                    // closed, surfacing intermittent "error sending request" failures
                    // under parallel test load.
                    if req_str.contains("/.well-known/openid-configuration") {
                        let body = format!(r#"{{"issuer":"{0}","jwks_uri":"{0}/jwks"}}"#, addr_owned);
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                    } else if req_str.contains("/jwks") {
                        request_count.fetch_add(1, Ordering::SeqCst);
                        if fail.load(Ordering::SeqCst) {
                            let _ = sock
                                .write_all(
                                    b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                                )
                                .await;
                        } else {
                            let n = count.load(Ordering::SeqCst);
                            let keys = (0..n)
                                .map(|i| format!(r#"{{"kty":"oct","kid":"k{}","k":"AAAAAA","alg":"HS256"}}"#, i))
                                .collect::<Vec<_>>()
                                .join(",");
                            let body = format!(r#"{{"keys":[{}]}}"#, keys);
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = sock.write_all(resp.as_bytes()).await;
                        }
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
        MockIdp {
            addr,
            jwks_should_fail,
            jwks_keys_count,
            jwks_request_count,
        }
    }

    fn test_config(addr: &str, ttl_ms: u64, min_refresh_ms: u64) -> JwksProviderConfig {
        JwksProviderConfig {
            discovery_url: format!("{}/.well-known/openid-configuration", addr),
            jwks_cache_ttl: Duration::from_millis(ttl_ms),
            jwks_min_refresh_interval: Duration::from_millis(min_refresh_ms),
            discovery_max_response_bytes: 65_536,
            jwks_max_response_bytes: 1_048_576,
            request_timeout: Duration::from_secs(2),
            component_label: "test",
        }
    }

    fn test_provider(cfg: JwksProviderConfig) -> Arc<JwksProvider<LeanDiscoveryDocument>> {
        let client = Arc::new(Client::builder().build().unwrap());
        JwksProvider::new(cfg, client, no_op_validator())
    }

    #[tokio::test]
    async fn discovery_cache_returns_one_immutable_snapshot() {
        let idp = spawn_mock_idp().await;
        let p = test_provider(test_config(&idp.addr, 60_000, 100));

        let fetched = p.get_or_fetch_discovery().await.unwrap();
        let cached_fetch = p.get_or_fetch_discovery().await.unwrap();
        let cached_read = p.cached_discovery().await.unwrap();
        assert!(Arc::ptr_eq(&fetched, &cached_fetch));
        assert!(Arc::ptr_eq(&fetched, &cached_read));
        assert_eq!(fetched.issuer(), idp.addr);

        let seeded = test_provider(test_config(&idp.addr, 60_000, 100));
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
        let idp = spawn_mock_idp().await;
        let p = test_provider(test_config(&idp.addr, 60_000, 100));
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
        let idp = spawn_mock_idp().await;
        let p = test_provider(test_config(&idp.addr, 50, 0));
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
        let idp = spawn_mock_idp().await;
        idp.jwks_should_fail.store(true, Ordering::SeqCst);
        let p = test_provider(test_config(&idp.addr, 60_000, 0));
        let err = p
            .get_jwks(false)
            .await
            .expect_err("must error with empty cache")
            .to_string();
        assert!(err.contains("503") || err.contains("status"), "got: {}", err);
    }

    #[tokio::test]
    async fn force_refresh_blocked_by_min_interval_returns_cached() {
        let idp = spawn_mock_idp().await;
        let p = test_provider(test_config(&idp.addr, 60_000, 60_000));
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
        let idp = spawn_mock_idp().await;
        let p = test_provider(test_config(&idp.addr, 60_000, 0));
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
        let idp = spawn_mock_idp().await;
        let p = test_provider(test_config(&idp.addr, 30, 5_000)); // 30ms TTL, 5s min_refresh
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
        let idp = spawn_mock_idp().await;
        let cfg = test_config(&idp.addr, 60_000, 0);
        let client = Arc::new(Client::builder().build().unwrap());
        let validator: DiscoveryValidator<LeanDiscoveryDocument> =
            Arc::new(|_| Err("synthetic validation failure".into()));
        let p = JwksProvider::new(cfg, client, validator);
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("validator should reject")
            .to_string();
        assert!(err.contains("synthetic"), "got: {}", err);
        assert!(
            p.cached_discovery().await.is_none(),
            "cache must remain empty after validator failure"
        );
    }

    // ---------------------------------------------------------------------
    // Exception-path tests using wiremock for fine-grained server control.
    // ---------------------------------------------------------------------

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a config pointing at a wiremock server with a custom request timeout.
    fn wm_config(addr: &str, ttl_ms: u64, min_refresh_ms: u64, timeout_ms: u64) -> JwksProviderConfig {
        JwksProviderConfig {
            discovery_url: format!("{}/.well-known/openid-configuration", addr),
            jwks_cache_ttl: Duration::from_millis(ttl_ms),
            jwks_min_refresh_interval: Duration::from_millis(min_refresh_ms),
            discovery_max_response_bytes: 65_536,
            jwks_max_response_bytes: 1_048_576,
            request_timeout: Duration::from_millis(timeout_ms),
            component_label: "test",
        }
    }

    /// Mount a happy-path discovery endpoint pointing jwks_uri at the same server.
    async fn mount_ok_discovery(server: &MockServer) {
        let body = serde_json::json!({
            "issuer": server.uri(),
            "jwks_uri": format!("{}/jwks", server.uri()),
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    fn build_provider(cfg: JwksProviderConfig) -> Arc<JwksProvider<LeanDiscoveryDocument>> {
        let client = Arc::new(Client::builder().build().unwrap());
        JwksProvider::new(cfg, client, no_op_validator())
    }

    #[tokio::test]
    async fn test_jwks_fetch_timeout_returns_error_when_no_cache() {
        // Discovery is fast, JWKS is slow → request-level timeout fires.
        let server = MockServer::start().await;
        mount_ok_discovery(&server).await;
        let body = serde_json::json!({
            "keys": [{"kty":"oct","kid":"k0","k":"AAAAAA","alg":"HS256"}],
        });
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(body)
                    .set_delay(Duration::from_millis(500)),
            )
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 100));
        let err = p
            .get_jwks(false)
            .await
            .expect_err("timeout must error with empty cache")
            .to_string();
        let lc = err.to_lowercase();
        assert!(
            lc.contains("timeout") || lc.contains("timed out") || lc.contains("jwks request failed"),
            "expected timeout-shaped error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_jwks_fetch_5xx_returns_error_when_no_cache() {
        // 5xx with no cache → hard fail (stale-while-revalidate has nothing to serve).
        let server = MockServer::start().await;
        mount_ok_discovery(&server).await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
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
        // First fetch succeeds (populates cache), second fetch sees 5xx → stale fallback.
        let server = MockServer::start().await;
        mount_ok_discovery(&server).await;
        let body_ok = serde_json::json!({
            "keys": [{"kty":"oct","kid":"k0","k":"AAAAAA","alg":"HS256"}],
        });
        // Two scoped responders: first OK (limit 1), then 5xx for everything after.
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_ok))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        // ttl very short so the 2nd call triggers a refetch.
        let p = build_provider(wm_config(&server.uri(), 30, 0, 2_000));
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
        let server = MockServer::start().await;
        mount_ok_discovery(&server).await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{not-valid-json"))
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
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
        // Provider explicitly rejects empty keys list ("JWKS endpoint returned empty keys").
        let server = MockServer::start().await;
        mount_ok_discovery(&server).await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"keys": []})))
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
        let err = p.get_jwks(false).await.expect_err("empty keys must error").to_string();
        assert!(err.contains("empty keys"), "expected 'empty keys' message, got: {err}");
    }

    #[tokio::test]
    async fn test_discovery_5xx_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
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
    async fn test_discovery_malformed_json_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("discovery malformed JSON must error")
            .to_string();
        let lc = err.to_lowercase();
        assert!(
            lc.contains("parse") || lc.contains("oidc discovery"),
            "expected parse error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_discovery_missing_jwks_uri_returns_error() {
        let server = MockServer::start().await;
        // Valid JSON but jwks_uri is empty.
        let body = serde_json::json!({
            "issuer": server.uri(),
            "jwks_uri": "",
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("missing jwks_uri must error")
            .to_string();
        assert!(
            err.contains("jwks_uri"),
            "expected 'jwks_uri' in error message, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_discovery_missing_issuer_returns_error() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "issuer": "",
            "jwks_uri": format!("{}/jwks", server.uri()),
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("missing issuer must error")
            .to_string();
        assert!(err.contains("issuer"), "expected 'issuer' in error message, got: {err}");
    }

    #[tokio::test]
    async fn test_discovery_unreachable_returns_error() {
        // Port 1 is reserved/unused — connection should fail fast.
        let cfg = JwksProviderConfig {
            discovery_url: "http://127.0.0.1:1/.well-known/openid-configuration".to_string(),
            jwks_cache_ttl: Duration::from_secs(60),
            jwks_min_refresh_interval: Duration::from_millis(0),
            discovery_max_response_bytes: 65_536,
            jwks_max_response_bytes: 1_048_576,
            request_timeout: Duration::from_millis(500),
            component_label: "test",
        };
        let p = build_provider(cfg);
        let err = p
            .get_or_fetch_discovery()
            .await
            .expect_err("unreachable discovery must error")
            .to_string();
        assert!(
            err.contains("Failed to fetch discovery document"),
            "expected fetch failure, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_jwks_kid_rotation_force_refresh_loads_new_keys() {
        // Simulate a kid rotation: initial JWKS has kid=k_old; after rotation only k_new.
        // Caller observes kid miss on k_old → invokes get_jwks(force_refresh=true) and gets k_new.
        let server = MockServer::start().await;
        mount_ok_discovery(&server).await;
        let body_old = serde_json::json!({
            "keys": [{"kty":"oct","kid":"k_old","k":"AAAAAA","alg":"HS256"}],
        });
        let body_new = serde_json::json!({
            "keys": [{"kty":"oct","kid":"k_new","k":"BBBBBB","alg":"HS256"}],
        });
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_old))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_new))
            .mount(&server)
            .await;
        // min_refresh=0 so force_refresh is not throttled.
        let p = build_provider(wm_config(&server.uri(), 60_000, 0, 2_000));
        let first = p.get_jwks(false).await.expect("initial fetch ok");
        assert_eq!(first.set.keys.len(), 1);
        let kid_first = first.set.keys[0].common.key_id.as_deref();
        assert_eq!(kid_first, Some("k_old"));

        let rotated = p.get_jwks(true).await.expect("force refresh fetch ok");
        assert_eq!(rotated.set.keys.len(), 1);
        assert!(!Arc::ptr_eq(&first.set, &rotated.set));
        let kid_rot = rotated.set.keys[0].common.key_id.as_deref();
        assert_eq!(kid_rot, Some("k_new"), "force_refresh should pick up rotated kid");
        // Old kid no longer present in the returned set.
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

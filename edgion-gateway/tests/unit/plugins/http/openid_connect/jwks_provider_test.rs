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

use super::*;

const TEST_SOURCE: &str = "test-source";

fn fixture_city_db() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb")
}

#[test]
fn structural_validation_accepts_a_real_database() {
    assert!(validate_structural(&fixture_city_db()).is_ok());
}

#[test]
fn structural_validation_rejects_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.mmdb");
    std::fs::write(&path, b"not an mmdb at all").unwrap();
    assert!(validate_structural(&path).is_err());
}

#[test]
fn structural_validation_rejects_a_truncated_database() {
    let full = std::fs::read(fixture_city_db()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("truncated.mmdb");
    std::fs::write(&path, &full[..full.len() / 2]).unwrap();
    assert!(validate_structural(&path).is_err());
}

#[test]
fn tmp_path_is_a_sibling_of_the_database() {
    let p = std::path::Path::new("/var/lib/edgion/geoip/db.mmdb");
    assert_eq!(
        tmp_path(p),
        std::path::PathBuf::from("/var/lib/edgion/geoip/db.mmdb.tmp")
    );
    assert_eq!(
        meta_path(p),
        std::path::PathBuf::from("/var/lib/edgion/geoip/db.mmdb.meta")
    );
}

/// Before the first success the backoff must be seconds, not the
/// steady-state ten minutes — a cold-started pod is unprotected meanwhile.
#[test]
fn cold_start_backoff_escalates_from_one_second_capped_at_thirty() {
    assert_eq!(cold_start_backoff(0), Duration::from_secs(1));
    assert_eq!(cold_start_backoff(1), Duration::from_secs(2));
    assert_eq!(cold_start_backoff(4), Duration::from_secs(16));
    assert_eq!(cold_start_backoff(5), Duration::from_secs(30));
    assert_eq!(cold_start_backoff(50), Duration::from_secs(30));
}

/// Loopback is allowed under `cfg!(test)` — the same escape hatch
/// `EgressResolver` uses, since the cycle tests below point `update_url` at
/// `127.0.0.1`. Production builds do not carry this escape hatch.
#[test]
fn ip_literal_loopback_update_url_is_allowed_under_test_escape_hatch() {
    assert!(classify_literal_destination("http://127.0.0.1:9999/db.mmdb").is_ok());
}

#[test]
fn ip_literal_link_local_metadata_address_is_rejected() {
    assert!(classify_literal_destination("http://169.254.169.254/db.mmdb").is_err());
}

#[test]
fn hostname_update_url_is_not_classified_here() {
    // Hostname targets are out of scope for this literal check; DNS-time
    // egress policy is a separate concern from this task.
    assert!(classify_literal_destination("https://mirror.internal/db.mmdb").is_ok());
}

/// Serve one canned response on an ephemeral port; returns the base URL.
async fn serve_once(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt as _};
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut throwaway = [0u8; 1024];
        let _ = sock.read(&mut throwaway).await;

        let status_line = match status {
            200 => "200 OK",
            304 => "304 Not Modified",
            other => panic!("serve_once: unsupported status {other} in test helper"),
        };
        let mut resp = format!("HTTP/1.1 {status_line}\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in &headers {
            resp.push_str(&format!("{k}: {v}\r\n"));
        }
        resp.push_str("\r\n");
        let _ = sock.write_all(resp.as_bytes()).await;
        if !body.is_empty() {
            let _ = sock.write_all(&body).await;
        }
        let _ = sock.shutdown().await;
    });
    format!("http://{addr}/")
}

fn updater_for(dir: &std::path::Path, url: String, max_db_bytes: u64) -> GeoIpUpdater {
    let cfg = GeoIpConfig {
        db_path: Some(dir.join("db.mmdb")),
        update_url: Some(url.into()),
        max_db_bytes,
        stall_timeout_sec: 5,
        download_deadline_sec: 30,
        ..Default::default()
    };
    // Production owns one process-global provider. Tests need independent
    // `'static` providers because updater cycles await network I/O and
    // must not mutate the global singleton across parallel tests.
    let provider = Box::leak(Box::new(GeoIpProvider::new()));
    GeoIpUpdater::new(cfg, provider).unwrap()
}

#[test]
fn authorization_header_is_sensitive_before_storage_and_clone() {
    let secret = "Bearer private-geoip-token";
    let value = sensitive_authorization_header("GEOIP_AUTH", secret).unwrap();
    assert!(value.is_sensitive());

    let cloned = value.clone();
    assert!(cloned.is_sensitive(), "HeaderValue clone must preserve sensitivity");
    assert!(
        !format!("{cloned:?}").contains(secret),
        "structured HeaderValue Debug must not expose the credential"
    );

    let client = reqwest::Client::new();
    let (request, sent_validator) = build_request(
        &client,
        "http://example.invalid/geo.mmdb",
        Some(&cloned),
        &UpdateState::default(),
        true,
    );
    assert!(!sent_validator);
    let request = request.build().unwrap();
    assert!(request.headers().get(AUTHORIZATION).unwrap().is_sensitive());
}

#[test]
fn invalid_authorization_header_error_names_only_the_environment_variable() {
    let raw = "Bearer private-geoip-token\ninvalid";
    let err = sensitive_authorization_header("GEOIP_AUTH", raw).unwrap_err();
    assert!(err.contains("GEOIP_AUTH"));
    assert!(!err.contains("private-geoip-token"));
    assert!(!err.contains(raw));
}

/// BLOCKER 5 regression: the precheck must require `max_db_bytes + 2x the
/// current database size`, not `3x max_db_bytes`. A 70 MB database against
/// a 512 MiB volume (comfortably 3x the *database*, per the docs) must not
/// be treated as needing `3 * max_db_bytes` (768 MiB at the default cap) —
/// that overshoot is exactly what wedged updates permanently for an
/// operator who sized storage off the docs' own advice.
#[test]
fn free_space_formula_is_max_db_bytes_plus_twice_current_size_not_3x_max_db_bytes() {
    let max_db_bytes = 268_435_456u64; // 256 MiB default cap
    let current_db_size = 70 * 1024 * 1024u64; // 70 MB live database

    let needed = free_space_needed(max_db_bytes, current_db_size);
    let old_3x_formula = max_db_bytes.saturating_mul(3);

    assert_eq!(needed, max_db_bytes + 2 * current_db_size);
    assert!(
        needed < old_3x_formula,
        "the corrected formula must not overshoot the old 3x max_db_bytes formula \
             for a database smaller than the cap: needed={needed}, old_3x={old_3x_formula}"
    );

    // A 512 MiB volume is comfortably 3x this 70 MB database (the docs'
    // sizing rule), so the corrected formula must fit it while the old
    // 3x-max_db_bytes formula (768 MiB) would not.
    let volume_bytes = 512 * 1024 * 1024u64;
    assert!(needed <= volume_bytes, "corrected formula must fit a docs-sized volume");
    assert!(
        old_3x_formula > volume_bytes,
        "old formula must NOT fit — this is the bug the fix corrects"
    );
}

/// With no database on disk yet (first-ever start), there is nothing to
/// double-hold, so the requirement falls back to `max_db_bytes` alone.
#[test]
fn free_space_formula_falls_back_to_max_db_bytes_with_no_existing_database() {
    assert_eq!(free_space_needed(268_435_456, 0), 268_435_456);
}

/// The free-space precheck failure must carry its own `disk` reason label,
/// distinguishable from a real network `fetch` failure — collapsing the two
/// (finding 6) sends an operator debugging the network for a storage
/// problem.
#[tokio::test]
async fn insufficient_free_space_fails_with_disk_reason_not_fetch() {
    let dir = tempfile::tempdir().unwrap();
    // A `max_db_bytes` far larger than any real volume's free space forces
    // the precheck to fail before any request is issued.
    let url = "http://127.0.0.1:1/db.mmdb".to_string();
    let u = updater_for(dir.path(), url, u64::MAX / 2);
    let mut state = UpdateState::default();
    let err = u.run_cycle(&mut state).await.unwrap_err();
    assert_eq!(
        err.reason(),
        "disk",
        "an oversized max_db_bytes must fail the precheck with reason=disk, not reason=fetch"
    );
}

/// BLOCKER 1 regression: `reqwest::Error`'s `Display` appends
/// " for url (...)", and `url::Url`'s `Display` echoes userinfo and query
/// verbatim. An `update_url` with an embedded credential must never survive
/// into `UpdateError::detail()`, since that string is logged at ERROR level
/// on every failed cycle (`run`, above). Port 1 is a deliberately
/// unreachable host (nothing listens there), which forces a connect-stage
/// `reqwest::Error` carrying the request URL.
#[tokio::test]
async fn fetch_error_detail_never_contains_the_update_url_or_its_credential() {
    let dir = tempfile::tempdir().unwrap();
    let secret_url = "http://svc:s3cr3t-token@127.0.0.1:1/db.mmdb?X-Amz-Signature=leaked".to_string();
    let u = updater_for(dir.path(), secret_url.clone(), 1 << 20);
    let mut state = UpdateState::default();
    let err = u.run_cycle(&mut state).await.unwrap_err();
    assert_eq!(err.reason(), "fetch");
    assert!(
        !err.detail().contains("s3cr3t-token") && !err.detail().contains("X-Amz-Signature"),
        "UpdateError detail must never carry the update_url or its credential, got: {}",
        err.detail()
    );
    assert!(
        !err.detail().contains(&secret_url),
        "UpdateError detail must not embed the full update_url, got: {}",
        err.detail()
    );
}

#[tokio::test]
async fn not_modified_with_usable_database_leaves_disk_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve_once(304, &[], Vec::new()).await;
    let u = updater_for(dir.path(), url, 1 << 20);
    std::fs::copy(fixture_city_db(), &u.db_path).unwrap();
    u.provider.load_from(&u.db_path).unwrap();
    let before = std::fs::read(&u.db_path).unwrap();
    let mut state = UpdateState {
        source_identity: Some(u.source_identity.clone()),
        etag: Some("\"abc\"".into()),
        last_modified: None,
    };
    assert!(matches!(u.run_cycle(&mut state).await, Ok(CycleOutcome::NotModified)));
    assert_eq!(std::fs::read(dir.path().join("db.mmdb")).unwrap(), before);
    assert!(!dir.path().join("db.mmdb.tmp").exists());
    assert_eq!(state.etag.as_deref(), Some("\"abc\""));
}

#[test]
fn invalid_not_modified_clears_validators_and_next_request_is_unconditional() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    let meta = meta_path(&db);
    let mut state = UpdateState {
        source_identity: Some(TEST_SOURCE.to_string()),
        etag: Some("\"stale\"".into()),
        last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
    };
    state.persist(&meta);

    let err = handle_not_modified_response(&mut state, &meta, true, false).unwrap_err();
    assert_eq!(err.reason(), "fetch");
    assert!(err.requires_cold_backoff());
    assert!(state.etag.is_none());
    assert!(state.last_modified.is_none());

    // A valid empty sidecar is loadable and intentionally suppresses the
    // database-mtime fallback after local eligibility returns.
    std::fs::write(&db, b"regular database placeholder").unwrap();
    let reloaded = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    assert!(reloaded.etag.is_none());
    assert!(reloaded.last_modified.is_none());

    let client = reqwest::Client::new();
    let (request, sent_validator) = build_request(&client, "http://example.invalid/geo.mmdb", None, &reloaded, true);
    assert!(!sent_validator);
    let request = request.build().unwrap();
    assert!(request.headers().get(IF_NONE_MATCH).is_none());
    assert!(request.headers().get(IF_MODIFIED_SINCE).is_none());
}

#[test]
fn not_modified_without_a_sent_validator_is_invalid_even_with_usable_local_state() {
    let dir = tempfile::tempdir().unwrap();
    let meta = meta_path(&dir.path().join("db.mmdb"));
    let mut state = UpdateState::default();
    let err = handle_not_modified_response(&mut state, &meta, false, true).unwrap_err();
    assert_eq!(err.reason(), "fetch");
    assert!(err.requires_cold_backoff());
}

#[test]
fn invalid_not_modified_forces_cold_backoff_even_with_an_active_provider() {
    let mut cold_attempt = 0;
    let delay = failure_sleep_base(
        true,
        UpdateError::InvalidNotModified("invalid 304".into()).requires_cold_backoff(),
        Duration::from_secs(600),
        &mut cold_attempt,
    );
    assert_eq!(delay, Duration::from_secs(1));
    assert_eq!(cold_attempt, 1);
}

/// The cap is enforced WHILE streaming; exceeding it must fail the cycle and
/// leave no partial file, because a stale .tmp wedges every later cycle.
#[tokio::test]
async fn oversized_body_fails_and_removes_tmp() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve_once(200, &[], vec![0u8; 4096]).await;
    let u = updater_for(dir.path(), url, 16);
    let mut state = UpdateState::default();
    let err = u.run_cycle(&mut state).await.unwrap_err();
    assert_eq!(err.reason(), "size_cap");
    assert!(!dir.path().join("db.mmdb.tmp").exists());
    assert!(!dir.path().join("db.mmdb").exists());
}

/// A gzip body written straight to .tmp would fail validation forever with an
/// opaque error, so reject the encoding up front instead.
#[tokio::test]
async fn gzip_content_encoding_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve_once(200, &[("content-encoding", "gzip")], vec![0u8; 32]).await;
    let u = updater_for(dir.path(), url, 1 << 20);
    let mut state = UpdateState::default();
    let err = u.run_cycle(&mut state).await.unwrap_err();
    assert_eq!(err.reason(), "encoding");
    assert!(!dir.path().join("db.mmdb.tmp").exists());
}

/// Half a broken database is worse than none.
#[tokio::test]
async fn failed_validation_never_renames() {
    let dir = tempfile::tempdir().unwrap();
    let url = serve_once(200, &[], b"not an mmdb".to_vec()).await;
    let u = updater_for(dir.path(), url, 1 << 20);
    let mut state = UpdateState::default();
    let err = u.run_cycle(&mut state).await.unwrap_err();
    assert_eq!(err.reason(), "validation");
    assert!(
        !dir.path().join("db.mmdb").exists(),
        "a rejected candidate must never be renamed into place"
    );
}

/// First-start `ensure_free_space` requires `max_db_bytes` on the temp
/// volume. Keep this cap fixture-scale so a constrained host tmpfs (well
/// under 1 GiB) can still exercise download, rename, swap, and `.meta`.
/// Disk exhaustion is covered by
/// `insufficient_free_space_fails_with_disk_reason_not_fetch`.
#[tokio::test]
async fn successful_cycle_renames_swaps_and_persists_meta() {
    let dir = tempfile::tempdir().unwrap();
    let body = std::fs::read(fixture_city_db()).unwrap();
    let url = serve_once(200, &[("etag", "\"v1\"")], body).await;
    let u = updater_for(dir.path(), url, 1 << 20);
    let mut state = UpdateState::default();
    assert!(matches!(u.run_cycle(&mut state).await, Ok(CycleOutcome::Swapped)));
    assert!(dir.path().join("db.mmdb").exists());
    assert!(!dir.path().join("db.mmdb.tmp").exists());
    assert!(dir.path().join("db.mmdb.meta").exists());
    assert_eq!(state.etag.as_deref(), Some("\"v1\""));
    assert!(state.matches_source(&u.source_identity));
    assert!(
        UpdateState::load(&meta_path(&u.db_path), &u.db_path, true, &u.source_identity)
            .matches_source(&u.source_identity)
    );
    assert!(u.provider.is_loaded());
}

/// A partial .tmp left by a crashed pod must not survive into the next cycle.
#[tokio::test]
async fn stale_tmp_is_removed_at_cycle_start() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("db.mmdb.tmp"), vec![0u8; 1024]).unwrap();
    let url = serve_once(304, &[], Vec::new()).await;
    let u = updater_for(dir.path(), url, 1 << 20);
    let mut state = UpdateState {
        source_identity: Some(TEST_SOURCE.to_string()),
        etag: Some("\"abc\"".into()),
        last_modified: None,
    };
    let _ = u.run_cycle(&mut state).await;
    assert!(!dir.path().join("db.mmdb.tmp").exists());
}

/// `load` seeds `last_modified` from the database file's mtime when the
/// `.meta` file is absent but the database already exists — the `emptyDir`
/// case where the validator was lost with the volume.
#[test]
fn load_seeds_last_modified_from_db_mtime_when_meta_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::write(&db, b"placeholder").unwrap();

    let state = UpdateState::load(&meta_path(&db), &db, true, TEST_SOURCE);
    assert!(state.etag.is_none());
    assert!(state.last_modified.is_some());
    assert!(state.matches_source(TEST_SOURCE));
}

/// With neither `.meta` nor a database present (first-ever start), there is
/// nothing to seed from.
#[test]
fn load_is_empty_when_neither_meta_nor_db_exist() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    let state = UpdateState::load(&meta_path(&db), &db, false, TEST_SOURCE);
    assert!(state.etag.is_none());
    assert!(state.last_modified.is_none());
}

/// `persist` followed by `load` round-trips the validator.
#[test]
fn persist_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::write(&db, b"regular database placeholder").unwrap();
    let meta = meta_path(&db);
    let state = UpdateState {
        source_identity: Some(TEST_SOURCE.to_string()),
        etag: Some("\"v2\"".into()),
        last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
    };
    state.persist(&meta);

    let reloaded = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    assert_eq!(reloaded.etag, state.etag);
    assert_eq!(reloaded.last_modified, state.last_modified);
}

#[test]
fn persisted_validators_require_both_regular_database_and_active_provider() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    let meta = meta_path(&db);
    let state = UpdateState {
        source_identity: Some(TEST_SOURCE.to_string()),
        etag: Some("\"v3\"".into()),
        last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
    };
    state.persist(&meta);

    let missing_db = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    assert!(missing_db.etag.is_none());
    assert!(missing_db.last_modified.is_none());

    std::fs::write(&db, b"regular database placeholder").unwrap();
    let restored_after_missing_db = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    assert!(restored_after_missing_db.etag.is_none());
    assert!(restored_after_missing_db.last_modified.is_none());

    state.persist(&meta);
    let inactive_provider = UpdateState::load(&meta, &db, false, TEST_SOURCE);
    assert!(inactive_provider.etag.is_none());
    assert!(inactive_provider.last_modified.is_none());

    let restored_after_inactive_provider = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    assert!(restored_after_inactive_provider.etag.is_none());
    assert!(restored_after_inactive_provider.last_modified.is_none());
}

#[test]
fn validator_context_requires_a_regular_file_and_active_provider() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.mmdb");
    assert!(!validator_context_ready(&missing, true));

    assert!(!validator_context_ready(dir.path(), true));

    let db = dir.path().join("db.mmdb");
    std::fs::write(&db, b"regular database placeholder").unwrap();
    assert!(!validator_context_ready(&db, false));
    assert!(validator_context_ready(&db, true));
}

#[test]
fn valid_empty_sidecar_suppresses_mtime_seed_when_context_is_usable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::copy(fixture_city_db(), &db).unwrap();
    let provider = Box::leak(Box::new(GeoIpProvider::new()));
    provider.load_from(&db).unwrap();

    let meta = meta_path(&db);
    UpdateState::default().persist(&meta);
    let state = UpdateState::load(&meta, &db, provider.is_loaded(), TEST_SOURCE);
    assert!(state.etag.is_none());
    assert!(state.last_modified.is_none());
}

#[test]
fn request_time_ineligibility_clears_memory_and_persists_empty_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    let meta = meta_path(&db);
    let mut state = UpdateState {
        source_identity: Some(TEST_SOURCE.to_string()),
        etag: Some("\"stale\"".into()),
        last_modified: Some("Tue, 15 Aug 2026 00:00:00 GMT".into()),
    };
    state.persist(&meta);

    prepare_validator_state(&mut state, &meta, false);
    assert!(state.etag.is_none());
    assert!(state.last_modified.is_none());

    std::fs::write(&db, b"regular database placeholder").unwrap();
    let restored = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    let client = reqwest::Client::new();
    let (request, sent_validator) = build_request(&client, "http://example.invalid/geo.mmdb", None, &restored, true);
    assert!(!sent_validator);
    let request = request.build().unwrap();
    assert!(request.headers().get(IF_NONE_MATCH).is_none());
    assert!(request.headers().get(IF_MODIFIED_SINCE).is_none());
}

/// A **hostname** `update_url` is exactly the case `classify_literal_destination`
/// cannot see (it only classifies IP literals; `Host::Domain(_)` short-circuits
/// to `Ok(())`). This proves the complementary half of the review fix: the
/// client actually resolves and dials `localhost` through `EgressResolver`
/// rather than through a bare `dns_resolver: None` reqwest default — if
/// `GeoIpUpdater::new` had not wired a resolver, or had wired a broken one,
/// this cycle would fail to connect at all.
///
/// What this does NOT cover: asserting that a hostname resolving to a
/// *blocked* address (private/link-local) is actually rejected. `loopback`
/// is allowed under the same `cfg!(test)` escape hatch `EgressResolver`
/// itself uses (see `ip_literal_loopback_update_url_is_allowed_under_test_escape_hatch`
/// above), and `EgressResolver::new` hardcodes `ResolverSpec::System` with no
/// test seam to inject a fake DNS answer for a non-loopback address, so a
/// true positive-rejection test is not achievable at this layer without
/// adding new resolver-injection plumbing that no other `EgressResolver`
/// consumer in this codebase has either.
#[tokio::test]
async fn hostname_update_url_resolves_and_completes_a_cycle_through_egress_resolver() {
    let dir = tempfile::tempdir().unwrap();
    let addr_url = serve_once(200, &[("etag", "\"v1\"")], b"not an mmdb".to_vec()).await;
    // serve_once binds 127.0.0.1:<port>; swap in a hostname so the request
    // goes through DNS resolution (and therefore through `EgressResolver`)
    // instead of `classify_literal_destination`'s IP-literal path.
    let hostname_url = addr_url.replacen("127.0.0.1", "localhost", 1);

    let u = updater_for(dir.path(), hostname_url, 1 << 20);
    let mut state = UpdateState::default();
    // The body is not a valid mmdb, so the cycle fails at structural
    // validation — but only after the resolver connected and the response
    // was fetched, which is exactly what proves EgressResolver is wired in.
    let err = u.run_cycle(&mut state).await.unwrap_err();
    assert_eq!(
        err.reason(),
        "validation",
        "a fetch/connect-stage failure would indicate the resolver never connected"
    );
}

fn assert_unconditional(state: &UpdateState) {
    let (request, sent_validator) = build_request(
        &reqwest::Client::new(),
        "https://example.invalid/geo.mmdb",
        None,
        state,
        true,
    );
    assert!(!sent_validator);
    let request = request.build().unwrap();
    assert!(request.headers().get(IF_NONE_MATCH).is_none());
    assert!(request.headers().get(IF_MODIFIED_SINCE).is_none());
}

#[test]
fn matching_source_restart_reuses_etag_before_last_modified() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::write(&db, b"regular database placeholder").unwrap();
    let meta = meta_path(&db);
    let mut state = UpdateState::default();
    state.record_swap(&meta, TEST_SOURCE, Some("\"same-tag\"".into()), Some("date".into()));
    let restored = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    let (request, sent) = build_request(
        &reqwest::Client::new(),
        "https://example.invalid/geo.mmdb",
        None,
        &restored,
        true,
    );
    assert!(sent);
    let request = request.build().unwrap();
    assert_eq!(request.headers()[IF_NONE_MATCH], "\"same-tag\"");
    assert!(request.headers().get(IF_MODIFIED_SINCE).is_none());
}

#[test]
fn changed_source_same_etag_stays_unconditional_across_failures_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::copy(fixture_city_db(), &db).unwrap();
    let provider = GeoIpProvider::new();
    provider.load_from(&db).unwrap();
    let before = std::fs::read(&db).unwrap();
    let source_a = source_identity("https://a.invalid/db.mmdb", None);
    let source_b = source_identity("https://b.invalid/db.mmdb", None);
    let meta = meta_path(&db);
    let mut state = UpdateState::default();
    state.record_swap(&meta, &source_a, Some("\"same-tag\"".into()), Some("date".into()));
    let mut changed = UpdateState::load(&meta, &db, provider.is_loaded(), &source_b);
    for _ in 0..2 {
        let eligible = validator_context_ready(&db, provider.is_loaded()) && changed.matches_source(&source_b);
        prepare_validator_state(&mut changed, &meta, eligible);
        assert_unconditional(&changed);
        changed = UpdateState::load(&meta, &db, provider.is_loaded(), &source_b);
    }
    assert!(!changed.matches_source(&source_b));
    assert_eq!(changed.source_identity.as_deref(), Some(source_a.as_str()));
    assert!(provider.is_loaded());
    assert_eq!(std::fs::read(&db).unwrap(), before);

    changed.record_swap(&meta, &source_b, Some("\"same-tag\"".into()), None);
    let promoted = UpdateState::load(&meta, &db, true, &source_b);
    assert!(promoted.matches_source(&source_b));
    assert_eq!(promoted.etag.as_deref(), Some("\"same-tag\""));
}

#[test]
fn legacy_or_malformed_existing_sidecars_never_seed_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::write(&db, b"regular database placeholder").unwrap();
    let meta = meta_path(&db);
    for bytes in [br#"{"etag":"same-tag","last_modified":"date"}"#.as_slice(), b"{partial"] {
        std::fs::write(&meta, bytes).unwrap();
        let state = UpdateState::load(&meta, &db, true, TEST_SOURCE);
        assert_unconditional(&state);
        assert!(!state.matches_source(TEST_SOURCE));
        assert_unconditional(&UpdateState::load(&meta, &db, true, TEST_SOURCE));
    }
}

#[test]
fn unreadable_existing_sidecar_never_seeds_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::write(&db, b"regular database placeholder").unwrap();
    let meta = meta_path(&db);
    std::fs::create_dir(&meta).unwrap();
    let state = UpdateState::load(&meta, &db, true, TEST_SOURCE);
    assert_unconditional(&state);
    assert!(!state.matches_source(TEST_SOURCE));
}

#[test]
fn source_identity_includes_url_and_resolved_authorization_without_plaintext() {
    let url = "https://user:private-password@example.invalid/db.mmdb?token=private-query";
    let auth = sensitive_authorization_header("GEOIP_AUTH", "Bearer private-token").unwrap();
    let same_auth = sensitive_authorization_header("RENAMED_ENV", "Bearer private-token").unwrap();
    let identity = source_identity(url, Some(&auth));
    assert_eq!(identity, source_identity(url, Some(&same_auth)));
    assert_eq!(identity.len(), 64);
    for alternative in [
        "https://other:private-password@example.invalid/db.mmdb?token=private-query",
        "https://user:private-password@example.invalid/db.mmdb?token=changed-query",
        "https://user:private-password@other.invalid/db.mmdb?token=private-query",
    ] {
        assert_ne!(identity, source_identity(alternative, Some(&auth)));
    }
    assert_ne!(identity, source_identity(url, None));
    assert_ne!(
        identity,
        source_identity(url, Some(&HeaderValue::from_static("Bearer changed-token")))
    );
    let state = UpdateState {
        source_identity: Some(identity),
        ..Default::default()
    };
    let serialized = serde_json::to_string(&state).unwrap();
    let debug = format!("{state:?}");
    for secret in [url, "private-password", "private-query", "private-token"] {
        assert!(!serialized.contains(secret));
        assert!(!debug.contains(secret));
    }
}

#[test]
fn failed_metadata_write_cannot_enable_surviving_old_source_validators() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.mmdb");
    std::fs::write(&db, b"regular database placeholder").unwrap();
    let meta = meta_path(&db);
    let mut state = UpdateState::default();
    state.record_swap(&meta, "source-a", Some("\"same-tag\"".into()), Some("date".into()));
    let impossible_meta = dir.path().join("missing-directory/db.mmdb.meta");
    state.record_swap(&impossible_meta, "source-b", Some("\"same-tag\"".into()), None);
    assert!(!impossible_meta.exists());
    let restored = UpdateState::load(&meta, &db, true, "source-b");
    assert_unconditional(&restored);
    assert!(!restored.matches_source("source-b"));
}

#[tokio::test]
async fn changed_source_disk_failure_preserves_last_good_provider_and_unconditional_restart() {
    let dir = tempfile::tempdir().unwrap();
    let updater = updater_for(dir.path(), "https://b.invalid/db.mmdb".into(), u64::MAX / 2);
    std::fs::copy(fixture_city_db(), &updater.db_path).unwrap();
    updater.provider.load_from(&updater.db_path).unwrap();
    let before = std::fs::read(&updater.db_path).unwrap();
    let meta = meta_path(&updater.db_path);
    let mut state = UpdateState::default();
    state.record_swap(&meta, "source-a", Some("\"same-tag\"".into()), Some("date".into()));
    for _ in 0..2 {
        let error = updater.run_cycle(&mut state).await.unwrap_err();
        assert_eq!(error.reason(), "disk");
        assert!(!state.matches_source(&updater.source_identity));
        assert_unconditional(&state);
        assert!(updater.provider.is_loaded());
        assert_eq!(std::fs::read(&updater.db_path).unwrap(), before);
        state = UpdateState::load(&meta, &updater.db_path, true, &updater.source_identity);
        assert_unconditional(&state);
    }
}

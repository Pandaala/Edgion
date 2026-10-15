//! Unit tests for the small pure helpers in this module, plus the
//! `ConfigSyncClient::new` security-branch decisions. `new` uses
//! `connect_lazy`, so it builds a Channel without any network I/O; the
//! auto_ca fail-close / plaintext branches are reachable without a live
//! server. The streaming paths (`start_watch_*`) still need integration
//! coverage.
use super::build_grpc_uri;
use super::ConfigSyncClient;
use edgion_common::conf_sync::config::{ConfSyncSecurityConfig, ConfSyncTlsConfig};
use std::time::Duration;

/// A loopback address that is never dialed (connect_lazy defers the connect).
const DUMMY_ADDR: &str = "127.0.0.1:65000";

/// An explicit `skip_tls` section — the only supported plaintext config, and the
/// cheapest way to build a client without touching cert files.
fn skip_tls_security() -> ConfSyncSecurityConfig {
    ConfSyncSecurityConfig {
        certs: vec![ConfSyncTlsConfig {
            name: None,
            cert: String::new(),
            key: String::new(),
            ca: String::new(),
            skip_tls: true,
        }],
        ..Default::default()
    }
}

fn installed_transport(installed: &super::InstalledChannel) -> super::PreparedClient {
    super::PreparedClient {
        client: installed.client.clone(),
        validity: installed.validity,
    }
}

#[test]
fn rotation_decision_matrix() {
    use super::{rotate_decision, RotateDecision};
    // never delivered ⇒ rotate immediately (bootstrap / kill -9 at startup).
    assert_eq!(rotate_decision(false, 1, false), RotateDecision::RotateNow);
    // delivered before, first failure ⇒ dwell first (reload flap protection, spec §7.4 H2).
    assert_eq!(rotate_decision(true, 1, false), RotateDecision::Dwell);
    // delivered before, dwell already spent ⇒ rotate.
    assert_eq!(rotate_decision(true, 2, false), RotateDecision::RotateNow);
    // demoted leader actively closed its stream ⇒ rotate regardless of history.
    assert_eq!(rotate_decision(true, 1, true), RotateDecision::RotateNow);
}

#[tokio::test]
async fn committed_candidate_changes_generation_and_rejects_duplicate() {
    let client = ConfigSyncClient::new(
        vec!["127.0.0.1:1".into(), "127.0.0.1:2".into()],
        "t".into(),
        Duration::from_secs(1),
        &skip_tls_security(),
    )
    .await
    .unwrap();
    let old = client.channels.installed.load_full();
    let (index, revision) = {
        let mut rotation = client.channels.swap_lock.lock().await;
        rotation.begin_search(old.index);
        rotation.candidate()
    };
    let validity = edgion_common::cert_util::LeafValidity {
        not_before: 10,
        not_after: 20,
    };
    let handle = super::PreparedClient {
        client: old.client.clone(),
        validity: Some(validity),
    };
    assert!(client
        .channels
        .commit(old.clone(), index, revision, handle.clone())
        .await
        .unwrap()
        .is_some());
    let new = client.channels.installed.load_full();
    assert_eq!(new.index, index);
    assert!(old.generation.is_cancelled());
    assert!(!new.generation.is_cancelled());
    assert_eq!(client.installed_cert_validity(), Some(validity));
    assert!(client
        .channels
        .commit(old, index, revision, handle)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        client.installed_cert_validity(),
        Some(validity),
        "stale publication cannot replace expiry metadata"
    );
}

#[tokio::test]
async fn same_address_after_full_lap_still_recycles_generation() {
    let client = ConfigSyncClient::new(
        vec!["127.0.0.1:1".into(), "127.0.0.1:2".into()],
        "t".into(),
        Duration::from_secs(1),
        &skip_tls_security(),
    )
    .await
    .unwrap();
    let old = client.channels.installed.load_full();
    let (index, revision) = {
        let mut rotation = client.channels.swap_lock.lock().await;
        rotation.begin_search(old.index);
        rotation.note_failure();
        rotation.candidate()
    };
    assert_eq!(index, old.index);
    assert!(client
        .channels
        .commit(old.clone(), index, revision, installed_transport(&old))
        .await
        .unwrap()
        .is_some());
    assert!(old.generation.is_cancelled());
}

#[tokio::test]
async fn rotate_is_noop_for_single_address() {
    let client = ConfigSyncClient::new(
        vec!["127.0.0.1:1".into()],
        "t".into(),
        Duration::from_secs(1),
        &skip_tls_security(),
    )
    .await
    .unwrap();
    let old = client.channels.installed.load_full();
    assert!(matches!(
        client.recover_step(old.clone()).await.unwrap(),
        super::RecoveryOutcome::Superseded
    ));
    assert!(std::sync::Arc::ptr_eq(&old, &client.channels.installed.load_full()));
    assert!(!old.generation.is_cancelled());
}

#[tokio::test]
async fn publication_finishes_after_caller_abort_and_notifies_last() {
    let client = ConfigSyncClient::new(
        vec!["127.0.0.1:1".into(), "127.0.0.1:2".into()],
        "t".into(),
        Duration::from_secs(1),
        &skip_tls_security(),
    )
    .await
    .unwrap();
    let old = client.channels.installed.load_full();
    let (index, revision) = {
        let mut rotator = client.channels.swap_lock.lock().await;
        rotator.begin_search(old.index);
        rotator.candidate()
    };
    let cache = client.config_client.publication_test_cache();
    let mut cache_guard = cache.grpc_client.write().await;
    *cache_guard = None;
    let manager = client.channels.clone();
    let expected = old.clone();
    let validity = edgion_common::cert_util::LeafValidity {
        not_before: 10,
        not_after: 20,
    };
    let caller = tokio::spawn(async move {
        let prepared = super::PreparedClient {
            client: expected.client.clone(),
            validity: Some(validity),
        };
        manager.commit(expected.clone(), index, revision, prepared).await
    });
    // The publisher holds the swap lock while it waits for our cache lock.
    tokio::time::timeout(Duration::from_secs(2), async {
        while client.channels.swap_lock.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(client.channels.is_current(&old));
    assert_eq!(
        client.installed_cert_validity(),
        old.validity,
        "pending cache publication must retain old expiry"
    );
    assert!(!old.generation.is_cancelled(), "notification must wait for every cache");
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    drop(cache_guard);
    tokio::time::timeout(Duration::from_secs(2), old.generation.cancelled())
        .await
        .unwrap();
    let installed = client.channels.installed.load_full();
    assert_eq!(installed.index, index);
    assert!(!installed.generation.is_cancelled());
    assert!(cache.grpc_client.read().await.is_some());
    assert_eq!(client.installed_cert_validity(), Some(validity));
}

#[tokio::test]
async fn local_tls_failure_does_not_skip_candidate_or_publish() {
    let mut client = ConfigSyncClient::new(
        vec!["127.0.0.1:1".into(), "127.0.0.1:2".into()],
        "t".into(),
        Duration::from_secs(1),
        &skip_tls_security(),
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    client.client_tls = Some(super::CertPaths {
        cert: dir.path().join("missing-cert"),
        key: dir.path().join("missing-key"),
        ca: dir.path().join("missing-ca"),
    });
    let old = client.channels.installed.load_full();
    assert!(matches!(
        client.recover_step(old.clone()).await.unwrap(),
        super::RecoveryOutcome::Retry { local: true, .. }
    ));
    let ticket = client.channels.swap_lock.lock().await.candidate();
    assert!(matches!(
        client.recover_step(old.clone()).await.unwrap(),
        super::RecoveryOutcome::Retry { local: true, .. }
    ));
    assert_eq!(client.channels.swap_lock.lock().await.candidate(), ticket);
    assert!(client.channels.is_current(&old));
    assert!(!old.generation.is_cancelled());
}

#[tokio::test]
async fn malformed_uri_anywhere_fails_before_shuffle_selection() {
    for addresses in [
        vec![DUMMY_ADDR.into(), "[invalid".into()],
        vec!["[invalid".into(), DUMMY_ADDR.into()],
    ] {
        assert!(
            ConfigSyncClient::new(addresses, "t".into(), Duration::from_secs(1), &skip_tls_security())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn queued_tls_reload_rebuilds_committed_address_and_fences_old_probe() {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let ca_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::from_ca_cert_pem(&ca.pem(), ca_key).unwrap();
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    let expected_validity = edgion_common::cert_util::parse_leaf_validity(cert.pem()).unwrap();
    let paths = super::CertPaths {
        cert: dir.path().join("cert.pem"),
        key: dir.path().join("key.pem"),
        ca: dir.path().join("ca.pem"),
    };
    std::fs::write(&paths.cert, cert.pem()).unwrap();
    std::fs::write(&paths.key, key.serialize_pem()).unwrap();
    std::fs::write(&paths.ca, ca.pem()).unwrap();
    let mut client = ConfigSyncClient::new(
        vec!["localhost:1".into(), "localhost:2".into()],
        "t".into(),
        Duration::from_secs(1),
        &skip_tls_security(),
    )
    .await
    .unwrap();
    client.client_tls = Some(paths);
    client.use_tls = true;
    let client = std::sync::Arc::new(client);
    let old = client.channels.installed.load_full();
    let (index, revision) = {
        let mut rotator = client.channels.swap_lock.lock().await;
        rotator.begin_search(old.index);
        rotator.candidate()
    };
    let cache = client.config_client.publication_test_cache();
    let cache_guard = cache.grpc_client.write().await;
    let manager = client.channels.clone();
    let expected = old.clone();
    let commit = tokio::spawn(async move {
        manager
            .commit(expected.clone(), index, revision, installed_transport(&expected))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while client.channels.swap_lock.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let reloader = client.clone();
    let reload = tokio::spawn(async move { reloader.reload_client_tls().await });
    drop(cache_guard);
    assert!(commit.await.unwrap().unwrap().is_some());
    reload.await.unwrap().unwrap();
    let installed = client.channels.installed.load_full();
    assert_eq!(
        installed.index, index,
        "reload must rebuild the address committed ahead of it"
    );
    assert_ne!(installed.address, old.address);
    assert!(old.generation.is_cancelled());
    assert!(!installed.generation.is_cancelled());
    assert_eq!(installed.validity, Some(expected_validity));
    assert!(client
        .channels
        .commit(old.clone(), index, revision, installed_transport(&old))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn auto_ca_with_omitted_tls_fails_closed_not_plaintext() {
    // auto_ca=true means "always mTLS". An omitted TLS block is a misconfig,
    // so `new` must refuse rather than silently downgrade to plaintext.
    let security = ConfSyncSecurityConfig {
        auto_ca: true,
        ..Default::default()
    };
    // ConfigSyncClient is not Debug, so avoid expect_err (needs Debug on the Ok type).
    let Err(err) = ConfigSyncClient::new(
        vec![DUMMY_ADDR.to_string()],
        "test".to_string(),
        Duration::from_secs(1),
        &security,
    )
    .await
    else {
        panic!("auto_ca + omitted TLS must fail closed");
    };
    let msg = format!("{err:?}");
    assert!(
        msg.contains("auto_ca"),
        "error must explain the auto_ca contract, got: {msg}"
    );
}

#[tokio::test]
async fn omitted_security_fails_closed_instead_of_plaintext() {
    // Fail-close (2026-07-14): an omitted `conf_sync_security` no longer connects in
    // plaintext — it refuses to start. `skip_tls: true` is the only plaintext path.
    let security = ConfSyncSecurityConfig::default();
    let Err(err) = ConfigSyncClient::new(
        vec![DUMMY_ADDR.to_string()],
        "test".to_string(),
        Duration::from_secs(1),
        &security,
    )
    .await
    else {
        panic!("omitted conf_sync_security must fail closed, not connect in plaintext");
    };
    let msg = format!("{err:?}");
    for needle in ["skip_tls", "auto_ca", "Secrets"] {
        assert!(msg.contains(needle), "error must mention {needle:?}, got: {msg}");
    }
}

#[tokio::test]
async fn skip_tls_without_auto_ca_still_connects_plaintext() {
    // The explicit escape hatch is untouched: skip_tls still yields a usable client.
    let security = skip_tls_security();
    let client = ConfigSyncClient::new(
        vec![DUMMY_ADDR.to_string()],
        "test".to_string(),
        Duration::from_secs(1),
        &security,
    )
    .await;
    assert!(
        client.is_ok(),
        "skip_tls must remain the explicit plaintext escape hatch"
    );
}

#[tokio::test]
async fn auto_ca_with_skip_tls_is_allowed_escape_hatch() {
    // skip_tls is the explicit emergency bypass; it overrides auto_ca (with a
    // loud WARN at runtime) and must NOT bail. No cert files are read.
    let security = ConfSyncSecurityConfig {
        auto_ca: true,
        certs: vec![ConfSyncTlsConfig {
            name: None,
            cert: String::new(),
            key: String::new(),
            ca: String::new(),
            skip_tls: true,
        }],
        ..Default::default()
    };
    let client = ConfigSyncClient::new(
        vec![DUMMY_ADDR.to_string()],
        "test".to_string(),
        Duration::from_secs(1),
        &security,
    )
    .await;
    assert!(
        client.is_ok(),
        "skip_tls must remain a usable escape hatch under auto_ca"
    );
}

#[test]
fn plain_addr_without_tls_uses_http_scheme() {
    assert_eq!(build_grpc_uri("controller:9443", false), "http://controller:9443");
}

#[test]
fn plain_addr_with_tls_uses_https_scheme() {
    assert_eq!(build_grpc_uri("controller:9443", true), "https://controller:9443");
}

#[test]
fn http_prefix_stripped_then_https_applied_when_tls_enabled() {
    // Operator may configure `http://...`; when TLS is on we still emit `https://`.
    assert_eq!(
        build_grpc_uri("http://controller:9443", true),
        "https://controller:9443"
    );
}

#[test]
fn https_prefix_stripped_then_http_applied_when_tls_disabled() {
    // Symmetric: a stale `https://` prefix must not survive when TLS is off.
    assert_eq!(
        build_grpc_uri("https://controller:9443", false),
        "http://controller:9443"
    );
}

#[test]
fn ipv4_with_port_round_trips() {
    assert_eq!(build_grpc_uri("127.0.0.1:9443", false), "http://127.0.0.1:9443");
    assert_eq!(build_grpc_uri("127.0.0.1:9443", true), "https://127.0.0.1:9443");
}

#[test]
fn matching_existing_scheme_is_idempotent() {
    // Same scheme requested → unchanged URI.
    assert_eq!(
        build_grpc_uri("https://controller:9443", true),
        "https://controller:9443"
    );
    assert_eq!(
        build_grpc_uri("http://controller:9443", false),
        "http://controller:9443"
    );
}

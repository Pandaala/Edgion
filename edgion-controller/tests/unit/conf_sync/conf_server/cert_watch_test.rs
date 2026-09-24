use super::*;
use crate::conf_sync::conf_server::tls_identity::{ServerIdentity, ServerIdentityHandle};
use crate::services::conf_sync_ca::Ca;
use std::sync::Arc;

// rustls 0.23 requires a process-level CryptoProvider before `ServerConfig::builder()`
// can be called (from_pems builds a rustls ServerConfig). Races harmlessly with other
// test modules doing the same `install_default()` — first writer wins.
fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[tokio::test]
async fn reload_swaps_on_valid_files_and_keeps_old_on_invalid() {
    install_test_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let leaf_a = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let write = |name: &str, content: &str| std::fs::write(dir.path().join(name), content).unwrap();
    write("tls.crt", &leaf_a.cert_pem);
    write("tls.key", leaf_a.key_pem.as_str());
    write("ca.crt", ca.ca_cert_pem());

    let handle = Arc::new(ServerIdentityHandle::new(
        ServerIdentity::from_pems(&leaf_a.cert_pem, &leaf_a.key_pem, ca.ca_cert_pem()).unwrap(),
    ));
    let paths = CertPaths {
        cert: dir.path().join("tls.crt"),
        key: dir.path().join("tls.key"),
        ca: dir.path().join("ca.crt"),
    };

    // Valid replacement -> swapped (longer-lived leaf B)
    let leaf_b = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(14))
        .unwrap();
    write("tls.crt", &leaf_b.cert_pem);
    write("tls.key", leaf_b.key_pem.as_str());
    let before = handle.load().not_after;
    reload_from_files(&paths, &handle).await.unwrap();
    assert!(handle.load().not_after > before);

    // Corrupt cert -> error, old config retained
    write("tls.crt", "garbage");
    let kept = handle.load().not_after;
    assert!(reload_from_files(&paths, &handle).await.is_err());
    assert_eq!(handle.load().not_after, kept);
}

/// Mirrors the gateway-side `restat_detects_mtime_or_len_change`: the backstop
/// must see a content change even when no inotify event was delivered.
#[test]
fn restat_detects_mtime_or_len_change() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f");
    std::fs::write(&p, "aaa").unwrap();
    let s1 = stat_signature(std::slice::from_ref(&p)).unwrap();
    std::fs::write(&p, "aaaa").unwrap();
    let s2 = stat_signature(std::slice::from_ref(&p)).unwrap();
    assert_ne!(s1, s2);
}

#[test]
fn signature_changed_decision_table() {
    let t0 = SystemTime::UNIX_EPOCH;
    let t1 = t0 + Duration::from_secs(1);
    let a = Some(vec![(t0, 10u64)]);
    let same = Some(vec![(t0, 10u64)]);
    let newer_mtime = Some(vec![(t1, 10u64)]);
    let new_len = Some(vec![(t0, 11u64)]);
    let missing: Option<Vec<(SystemTime, u64)>> = None;

    // No change -> no reload (a no-op fs event must not churn the identity).
    assert!(!signature_changed(&a, &same));
    // mtime moved / length moved -> reload.
    assert!(signature_changed(&a, &newer_mtime));
    assert!(signature_changed(&a, &new_len));
    // Files vanished (stat error) and came back -> both directions are a change.
    assert!(signature_changed(&a, &missing));
    assert!(signature_changed(&missing, &a));
    // Still-missing -> still no reload (nothing to read; reload would just fail).
    assert!(!signature_changed(&missing, &missing));
}

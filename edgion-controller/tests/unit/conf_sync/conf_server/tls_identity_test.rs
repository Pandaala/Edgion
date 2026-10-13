use super::*;
use crate::services::conf_sync_ca::Ca;

// rustls 0.23 requires a process-level CryptoProvider before `ServerConfig::builder()`
// can be called. Installing it here (ring) races harmlessly with other test modules
// (e.g. edgion_common::grpc_tls) doing the same `install_default()` — the first writer
// wins and the rest are no-ops, which is fine since either provider satisfies this code.
fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[test]
fn from_pems_builds_h2_config_with_metadata() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let leaf = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let id = ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap();
    assert_eq!(id.config.alpn_protocols, vec![b"h2".to_vec()]);
    assert!(id.not_after > id.not_before);
    assert!(!id.serial.is_empty());
    // The trust anchor the verifier was built from is retained verbatim — this is
    // what lets `identity_sync` detect a same-key root RENEWAL (new root cert, old
    // leaves still chain to it), which no chain check can see.
    assert_eq!(id.ca_cert_pem, ca.ca_cert_pem());
}

/// A same-key root renewal yields a DIFFERENT root certificate that the old leaf
/// still chains to. The retained `ca_cert_pem` is the only thing that distinguishes
/// the two — the property `identity_sync`'s root-drift arm relies on.
#[test]
fn same_key_renewal_changes_the_retained_root_bytes() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let leaf = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let renewed = ca.renew_root_same_key(time::Duration::days(60)).unwrap();
    // Same key => the old leaf still chains to the renewed root...
    assert!(crate::services::conf_sync_ca::cert_util::chains_to(
        &leaf.cert_pem,
        renewed.ca_cert_pem()
    ));
    // ...but the root CERTIFICATE bytes differ, and that is what we store.
    let id = ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap();
    assert_ne!(id.ca_cert_pem.trim(), renewed.ca_cert_pem().trim());
}

#[test]
fn from_pems_rejects_mismatched_material() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let other = Ca::generate_root(time::Duration::days(30)).unwrap();
    let leaf = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    assert!(ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, other.ca_cert_pem()).is_err());
}

// `global_handle()` is a process-wide `OnceLock` shared across every test
// in this binary. No other test in the crate sets it (checked via grep),
// so this test is the sole writer — but to stay race-safe against test
// execution order and any future test that also sets it, we only ensure
// it is set (tolerating an already-set handle from elsewhere) and then
// assert on the post-condition, rather than asserting it starts unset.
#[test]
fn live_server_cert_pem_reads_through_the_handle() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let leaf = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let id = ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap();
    let handle = Arc::new(ServerIdentityHandle::new(id));
    let _ = global_handle().set(handle);

    // Whichever identity ended up installed first (this test's, or another
    // test's if one races ahead), the handle must be set and readable.
    assert!(live_server_cert_pem().is_some());
}

/// Routine leaf rotation (SAME root): the trust token must SURVIVE. Established
/// gateway streams keep running — that is what makes rotation zero-disruption.
#[test]
fn same_root_store_keeps_established_sessions_alive() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let a = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let b = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(14))
        .unwrap();
    let handle =
        ServerIdentityHandle::new(ServerIdentity::from_pems(&a.cert_pem, &a.key_pem, ca.ca_cert_pem()).unwrap());
    let (old_config, token) = handle.handshake_material();
    let generation = handle.trust_generation();

    handle.store(ServerIdentity::from_pems(&b.cert_pem, &b.key_pem, ca.ca_cert_pem()).unwrap());

    assert!(
        !token.is_cancelled(),
        "a same-root leaf rotation must NOT kill sessions"
    );
    assert_eq!(handle.trust_generation(), generation);
    let (new_config, _) = handle.handshake_material();
    assert!(!Arc::ptr_eq(&old_config, &new_config));
    assert!(Arc::ptr_eq(&new_config, &handle.load().config));
    assert!(
        Arc::ptr_eq(&token, &handle.handshake_material().1),
        "token must be unchanged"
    );
}

/// Trust-ROOT change (emergency re-key / BYO-root swap): the old token must be
/// cancelled, the generation bumped, and a fresh uncancelled token installed.
/// This is the revocation.
#[test]
fn root_change_store_cancels_established_sessions() {
    install_test_crypto_provider();
    let ca_a = Ca::generate_root(time::Duration::days(30)).unwrap();
    let ca_b = Ca::generate_root(time::Duration::days(30)).unwrap();
    let a = ca_a
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let b = ca_b
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let handle =
        ServerIdentityHandle::new(ServerIdentity::from_pems(&a.cert_pem, &a.key_pem, ca_a.ca_cert_pem()).unwrap());
    let (old_config, old) = handle.handshake_material();
    assert!(Arc::ptr_eq(&old_config, &handle.load().config));
    assert_eq!(handle.trust_generation(), 0);

    handle.store(ServerIdentity::from_pems(&b.cert_pem, &b.key_pem, ca_b.ca_cert_pem()).unwrap());

    assert!(old.is_cancelled(), "a root change MUST tear down old sessions");
    assert_eq!(handle.trust_generation(), 1);
    // The retained pre-change handshake cannot borrow the new root's live token.
    let (fresh_config, fresh) = handle.handshake_material();
    assert!(!Arc::ptr_eq(&old_config, &fresh_config));
    assert!(Arc::ptr_eq(&fresh_config, &handle.load().config));
    assert!(!fresh.is_cancelled(), "connections accepted after the re-key must live");
    assert!(!Arc::ptr_eq(&old, &fresh));
}

/// A same-key root RENEWAL changes the root CERTIFICATE bytes, so it counts as a
/// root change here (conservative: the anchor is a different certificate).
#[test]
fn same_key_root_renewal_counts_as_a_root_change() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let leaf = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let renewed = ca.renew_root_same_key(time::Duration::days(60)).unwrap();
    let handle =
        ServerIdentityHandle::new(ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap());
    let old = handle.handshake_material().1;
    let leaf2 = renewed
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    handle.store(ServerIdentity::from_pems(&leaf2.cert_pem, &leaf2.key_pem, renewed.ca_cert_pem()).unwrap());
    assert!(old.is_cancelled());
    assert_eq!(handle.trust_generation(), 1);
}

#[test]
fn handle_swaps_atomically() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let a = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(7))
        .unwrap();
    let b = ca
        .sign_server(&["localhost".to_string()], time::Duration::days(14))
        .unwrap();
    let handle =
        ServerIdentityHandle::new(ServerIdentity::from_pems(&a.cert_pem, &a.key_pem, ca.ca_cert_pem()).unwrap());
    let na_a = handle.load().not_after;
    handle.store(ServerIdentity::from_pems(&b.cert_pem, &b.key_pem, ca.ca_cert_pem()).unwrap());
    assert!(handle.load().not_after > na_a);
}

#[test]
fn concurrent_same_root_stores_publish_one_live_trust_generation() {
    install_test_crypto_provider();
    let old_ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let new_ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let make_identity = |ca: &Ca| {
        let leaf = ca
            .sign_server(&["localhost".to_string()], time::Duration::days(7))
            .unwrap();
        ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap()
    };
    let handle = ServerIdentityHandle::new(make_identity(&old_ca));
    let (old_config, old_token) = handle.handshake_material();
    let identities: Vec<_> = (0..8).map(|_| make_identity(&new_ca)).collect();
    let configs: Vec<_> = identities.iter().map(|id| id.config.clone()).collect();
    let barrier = std::sync::Barrier::new(identities.len());
    let tokens = std::thread::scope(|scope| {
        let writers: Vec<_> = identities
            .into_iter()
            .map(|id| {
                let handle = &handle;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    handle.store(id);
                    handle.handshake_material().1
                })
            })
            .collect();
        writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect::<Vec<_>>()
    });
    let (config, token) = handle.handshake_material();
    assert_eq!(handle.trust_generation(), 1);
    assert!(old_token.is_cancelled());
    assert!(!Arc::ptr_eq(&old_config, &config));
    assert!(configs.iter().any(|candidate| Arc::ptr_eq(candidate, &config)));
    assert!(Arc::ptr_eq(&config, &handle.load().config));
    assert_eq!(handle.load().ca_cert_pem, new_ca.ca_cert_pem());
    assert!(!token.is_cancelled());
    assert!(tokens.iter().all(|captured| Arc::ptr_eq(captured, &token)));
}

fn conditional_identity(ca: &Ca) -> ServerIdentity {
    let leaf = ca.sign_server(&["localhost".into()], time::Duration::days(7)).unwrap();
    ServerIdentity::from_pems(&leaf.cert_pem, &leaf.key_pem, ca.ca_cert_pem()).unwrap()
}

#[test]
fn conditional_store_preserves_rotation_and_revocation_contracts() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let handle = ServerIdentityHandle::new(conditional_identity(&ca));
    let expected = handle.load();
    let token = handle.handshake_material().1;
    assert!(handle.store_if_current(&expected, conditional_identity(&ca)));
    assert!(!Arc::ptr_eq(&expected, &handle.load()));
    assert!(Arc::ptr_eq(&token, &handle.handshake_material().1));
    assert!(!token.is_cancelled());
    assert_eq!(handle.trust_generation(), 0);

    let other = Ca::generate_root(time::Duration::days(30)).unwrap();
    assert!(handle.store_if_current(&handle.load(), conditional_identity(&other)));
    assert!(token.is_cancelled());
    assert!(!handle.handshake_material().1.is_cancelled());
    assert_eq!(handle.trust_generation(), 1);
}

#[test]
fn stale_conditional_store_cannot_change_the_current_trust_generation() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let other = Ca::generate_root(time::Duration::days(30)).unwrap();
    let handle = ServerIdentityHandle::new(conditional_identity(&ca));
    let expected = handle.load();
    let candidate = conditional_identity(&ca);
    // A newer publication wins after candidate preparation but before commit.
    handle.store(conditional_identity(&other));
    let current = handle.load();
    let token = handle.handshake_material().1;
    let generation = handle.trust_generation();
    assert!(!handle.store_if_current(&expected, candidate));
    assert!(Arc::ptr_eq(&current, &handle.load()));
    assert!(Arc::ptr_eq(&token, &handle.handshake_material().1));
    assert!(!token.is_cancelled());
    assert_eq!(handle.trust_generation(), generation);
}

#[test]
fn concurrent_conditional_candidates_have_exactly_one_winner() {
    install_test_crypto_provider();
    let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
    let handle = ServerIdentityHandle::new(conditional_identity(&ca));
    let expected = handle.load();
    let old_token = handle.handshake_material().1;
    let candidates: Vec<_> = (0..2)
        .map(|_| conditional_identity(&Ca::generate_root(time::Duration::days(30)).unwrap()))
        .collect();
    let barrier = std::sync::Barrier::new(candidates.len());
    let outcomes = std::thread::scope(|scope| {
        let writers: Vec<_> = candidates
            .into_iter()
            .map(|candidate| {
                let handle = &handle;
                let expected = &expected;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    let config = candidate.config.clone();
                    (handle.store_if_current(expected, candidate), config)
                })
            })
            .collect();
        writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|(installed, _)| *installed).count(), 1);
    let winner = &outcomes.iter().find(|(installed, _)| *installed).unwrap().1;
    assert!(Arc::ptr_eq(winner, &handle.load().config));
    assert_eq!(handle.trust_generation(), 1);
    assert!(old_token.is_cancelled());
    assert!(!handle.handshake_material().1.is_cancelled());
}

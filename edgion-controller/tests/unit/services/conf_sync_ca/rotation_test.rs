use super::*;

#[tokio::test]
async fn rejected_root_renewal_stops_before_leaf_publication() {
    use crate::services::conf_sync_ca::{build_secret, Ca, CA_SECRET};
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    for status in [404, 409, 422] {
        let root = Ca::generate_root(time::Duration::days(30)).unwrap();
        let mut source = build_secret(
            CA_SECRET,
            "test",
            BTreeMap::from([
                ("ca.crt".into(), root.ca_cert_pem().as_bytes().to_vec()),
                ("ca.key".into(), root.ca_key_pem().as_bytes().to_vec()),
            ]),
        );
        source.metadata.uid = Some("old-root".into());
        source.metadata.resource_version = Some("10".into());
        let calls = Arc::new(AtomicUsize::new(0));
        let captured = calls.clone();
        let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let step = captured.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                request.uri().path(),
                format!("/api/v1/namespaces/test/secrets/{CA_SECRET}"),
                "rejected root renewal must never write the leaf Secret"
            );
            let (code, response) = match step {
                0 => {
                    assert_eq!(request.method(), http::Method::GET);
                    (200, serde_json::to_value(&source).unwrap())
                }
                1 => {
                    assert_eq!(request.method(), http::Method::PATCH);
                    (
                        status,
                        serde_json::json!({"apiVersion":"v1", "kind":"Status",
                        "status":"Failure", "code":status, "reason":"Rejected", "message":"source changed"}),
                    )
                }
                _ => panic!("unexpected request after renewal rejection"),
            };
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(code)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&response).unwrap()))
                        .unwrap(),
                )
            }
        });
        let api = Api::namespaced(Client::new(transport, "test"), "test");
        let params = Params {
            ns: "test".into(),
            sans: vec!["localhost".into()],
            rotation: ConfSyncRotationConfig::default(),
        };
        let error = rotate(&api, &params, RotateScope::Ca, "test", current_generation())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "conditionally renew CA secret");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

// ================================================================
// decide_trigger — full 2^3 truth table (8 cases → 4 outcomes)
// ================================================================

#[test]
fn decide_trigger_truth_table() {
    struct Case {
        initialized: bool,
        leader: bool,
        running: bool,
        expected: TriggerResult,
    }
    let cases = [
        // Not initialized dominates regardless of leader/running.
        Case {
            initialized: false,
            leader: false,
            running: false,
            expected: TriggerResult::NotConfigured,
        },
        Case {
            initialized: false,
            leader: false,
            running: true,
            expected: TriggerResult::NotConfigured,
        },
        Case {
            initialized: false,
            leader: true,
            running: false,
            expected: TriggerResult::NotConfigured,
        },
        Case {
            initialized: false,
            leader: true,
            running: true,
            expected: TriggerResult::NotConfigured,
        },
        // Initialized but not leader → NotLeader (running irrelevant).
        Case {
            initialized: true,
            leader: false,
            running: false,
            expected: TriggerResult::NotLeader,
        },
        Case {
            initialized: true,
            leader: false,
            running: true,
            expected: TriggerResult::NotLeader,
        },
        // Leader but worker not running yet → LeaderNotReady.
        Case {
            initialized: true,
            leader: true,
            running: false,
            expected: TriggerResult::LeaderNotReady,
        },
        // Leader and running → Accepted.
        Case {
            initialized: true,
            leader: true,
            running: true,
            expected: TriggerResult::Accepted,
        },
    ];
    for c in &cases {
        assert_eq!(
            decide_trigger(c.initialized, c.leader, c.running),
            c.expected,
            "initialized={} leader={} running={}",
            c.initialized,
            c.leader,
            c.running
        );
    }
}

// ================================================================
// RetryBackoff — base 30s, x2, 600s cap; ready()/bump()/reset()
// ================================================================

#[test]
fn retry_backoff_progression_and_readiness() {
    let base = Duration::from_secs(30);
    let cap = Duration::from_secs(600);
    let t0 = Instant::now();
    let mut b = RetryBackoff::new(base, cap);

    // Fresh backoff is immediately ready.
    assert!(b.ready(t0), "fresh backoff must be ready");

    // First failure: 30s window, then not-ready until it elapses.
    b.bump(t0);
    assert!(!b.ready(t0 + Duration::from_secs(29)), "must not be ready at 29s");
    assert!(b.ready(t0 + Duration::from_secs(30)), "must be ready at 30s");

    // Doubling sequence of imposed windows: 30 → 60 → 120 → 240 → 480 → 600 (cap).
    let mut b = RetryBackoff::new(base, cap);
    let expected = [30u64, 60, 120, 240, 480, 600, 600, 600];
    let mut cursor = t0;
    for (i, want) in expected.iter().enumerate() {
        b.bump(cursor);
        let w = Duration::from_secs(*want);
        assert!(
            !b.ready(cursor + w - Duration::from_secs(1)),
            "bump #{i}: must not be ready just before {want}s"
        );
        assert!(b.ready(cursor + w), "bump #{i}: must be ready at {want}s");
        // Advance the clock past this window for the next bump.
        cursor += w;
    }

    // reset() restores immediate readiness AND the base delay.
    b.reset();
    assert!(b.ready(cursor), "reset must restore immediate readiness");
    b.bump(cursor);
    assert!(
        !b.ready(cursor + Duration::from_secs(29)),
        "post-reset window is base (30s)"
    );
    assert!(
        b.ready(cursor + Duration::from_secs(30)),
        "post-reset window is base (30s)"
    );
}

// ================================================================
// should_log_skip — once per distinct firing
// ================================================================

#[test]
fn should_log_skip_dedups_per_firing() {
    use time::macros::datetime;
    let a = datetime!(2026-01-04 03:00 UTC);
    let b = datetime!(2026-01-11 03:00 UTC);
    let mut last: Option<OffsetDateTime> = None;

    // First sight of A → log.
    assert!(should_log_skip(&mut last, a));
    // Repeat A → suppressed.
    assert!(!should_log_skip(&mut last, a));
    assert!(!should_log_skip(&mut last, a));
    // New firing B → log.
    assert!(should_log_skip(&mut last, b));
    assert!(!should_log_skip(&mut last, b));
    // Back to A (last was B) → log again.
    assert!(should_log_skip(&mut last, a));
}

// ================================================================
// out_of_sync_reason — trust-root chain-consistency (emergency re-key /
// out-of-band BYO-root swap self-heal)
// ================================================================

use crate::services::conf_sync_ca::Ca;

fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[test]
fn out_of_sync_reason_table() {
    // `validate_identity` (arm 4) needs a rustls CryptoProvider.
    install_test_crypto_provider();

    let ca_a = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let ca_b = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let client_a = ca_a.sign_client(time::Duration::days(30)).unwrap();
    let server_a = ca_a
        .sign_server(&["example.svc".into()], time::Duration::days(30))
        .unwrap();
    let client_b = ca_b.sign_client(time::Duration::days(30)).unwrap();
    let server_b = ca_b
        .sign_server(&["example.svc".into()], time::Duration::days(30))
        .unwrap();

    // In-sync: both leaves chain, the Secret's ca.crt IS the current root, and
    // the Secret's cert/key pair validates -> None.
    assert!(out_of_sync_reason(
        &client_a.cert_pem,
        &client_a.key_pem,
        ca_a.ca_cert_pem(),
        &server_a.cert_pem,
        ca_a.ca_cert_pem()
    )
    .is_none());

    // Stale client leaf only -> Some(client...).
    let reason = out_of_sync_reason(
        &client_b.cert_pem,
        &client_b.key_pem,
        ca_b.ca_cert_pem(),
        &server_a.cert_pem,
        ca_a.ca_cert_pem(),
    );
    assert_eq!(reason, Some("client leaf does not chain to current CA"));

    // Stale server leaf only -> Some(server...).
    let reason = out_of_sync_reason(
        &client_a.cert_pem,
        &client_a.key_pem,
        ca_a.ca_cert_pem(),
        &server_b.cert_pem,
        ca_a.ca_cert_pem(),
    );
    assert_eq!(reason, Some("server identity does not chain to current CA"));

    // Both stale -> client reason wins (checked first).
    let reason = out_of_sync_reason(
        &client_b.cert_pem,
        &client_b.key_pem,
        ca_b.ca_cert_pem(),
        &server_b.cert_pem,
        ca_a.ca_cert_pem(),
    );
    assert_eq!(reason, Some("client leaf does not chain to current CA"));

    // Renewed-same-key root, everything rewritten consistently -> None (normal
    // renewal never trips the check).
    let renewed = ca_a.renew_root_same_key(time::Duration::days(3650)).unwrap();
    let client_renewed = renewed.sign_client(time::Duration::days(30)).unwrap();
    let server_renewed = renewed
        .sign_server(&["example.svc".into()], time::Duration::days(30))
        .unwrap();
    assert!(out_of_sync_reason(
        &client_renewed.cert_pem,
        &client_renewed.key_pem,
        renewed.ca_cert_pem(),
        &server_renewed.cert_pem,
        renewed.ca_cert_pem()
    )
    .is_none());

    // ARM 3 — TORN WRITE: `renew_ca` landed (root is now the renewed cert) but
    // `write_leaf` did not, so the leaf Secret still carries the OLD ca.crt. Both
    // leaves still chain (same key!), the CA was just renewed so `ca_renewal_due`
    // is false, and the policy is not due — arms 1/2 see nothing. Arm 3 is the
    // ONLY thing that catches it.
    let reason = out_of_sync_reason(
        &client_a.cert_pem,
        &client_a.key_pem,
        ca_a.ca_cert_pem(), // stale: the pre-renewal root cert
        &server_a.cert_pem,
        renewed.ca_cert_pem(), // current root
    );
    assert_eq!(reason, Some("leaf secret ca.crt is not the current root cert"));

    // Arm 3 tolerates trailing-whitespace differences in the PEM encoding.
    let padded = format!("{}\n\n", ca_a.ca_cert_pem());
    assert!(out_of_sync_reason(
        &client_a.cert_pem,
        &client_a.key_pem,
        &padded,
        &server_a.cert_pem,
        ca_a.ca_cert_pem()
    )
    .is_none());

    // ARM 4 — the Secret's client.key does not match its client.crt (bad manual
    // edit / partially-applied Secret). Chains are fine, ca.crt is the current
    // root; only `validate_identity` catches it.
    let other_client = ca_a.sign_client(time::Duration::days(30)).unwrap();
    let reason = out_of_sync_reason(
        &client_a.cert_pem,
        &other_client.key_pem, // mismatched key
        ca_a.ca_cert_pem(),
        &server_a.cert_pem,
        ca_a.ca_cert_pem(),
    );
    assert_eq!(reason, Some("leaf secret identity fails validation"));

    // ARM 4 — garbage key material.
    let reason = out_of_sync_reason(
        &client_a.cert_pem,
        "not a key",
        ca_a.ca_cert_pem(),
        &server_a.cert_pem,
        ca_a.ca_cert_pem(),
    );
    assert_eq!(reason, Some("leaf secret identity fails validation"));
}

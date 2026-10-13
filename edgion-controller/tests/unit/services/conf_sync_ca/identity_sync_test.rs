use super::*;
use time::macros::datetime;

/// The root cert this identity was built with, and the root currently in the CA
/// Secret. Distinct bytes model a same-key renewal (or any root-cert swap).
const ROOT_A: &str = "-----BEGIN CERTIFICATE-----\nROOT-A\n-----END CERTIFICATE-----\n";
const ROOT_B: &str = "-----BEGIN CERTIFICATE-----\nROOT-B\n-----END CERTIFICATE-----\n";

#[test]
fn identity_resign_reason_table() {
    let now = datetime!(2026-06-15 12:00 UTC);
    let emergency = time::Duration::hours(24);
    let ratio = 0.5;

    struct Case {
        name: &'static str,
        chains: bool,
        identity_ca: &'static str,
        current_ca: &'static str,
        window: (OffsetDateTime, OffsetDateTime),
        expected: Option<&'static str>,
    }
    // A fresh leaf: 1 day old, 364 days left (well before half-life).
    let fresh = (now - time::Duration::days(1), now + time::Duration::days(364));
    // Past half-life: 200 days old of a 365-day window.
    let half_life = (now - time::Duration::days(200), now + time::Duration::days(165));
    // Inside the 24h emergency window.
    let dying = (now - time::Duration::days(364), now + time::Duration::hours(3));
    // Exactly AT the half-life instant (boundary: `>=` must fire).
    let at_ratio = (now - time::Duration::days(100), now + time::Duration::days(100));
    // Exactly AT the emergency boundary (`<=` must fire).
    let at_emergency = (now - time::Duration::days(300), now + emergency);

    let cases = [
        Case {
            name: "healthy leaf chaining to the current root",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: ROOT_A,
            window: fresh,
            expected: None,
        },
        Case {
            name: "does not chain (emergency re-key) — highest priority",
            chains: false,
            identity_ca: ROOT_A,
            current_ca: ROOT_B,
            window: fresh,
            expected: Some("server identity does not chain to the current root"),
        },
        Case {
            name: "does not chain wins over the emergency window",
            chains: false,
            identity_ca: ROOT_A,
            current_ca: ROOT_B,
            window: dying,
            expected: Some("server identity does not chain to the current root"),
        },
        Case {
            // The same-key-renewal blind spot: the leaf still chains (same root key),
            // but the root CERT changed, so this replica's verifier holds a stale
            // anchor. Must re-sign.
            name: "chains, healthy window, but the root cert changed (same-key renewal)",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: ROOT_B,
            window: fresh,
            expected: Some("server identity trust anchor is not the current root cert"),
        },
        Case {
            name: "root-cert drift outranks the emergency window",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: ROOT_B,
            window: dying,
            expected: Some("server identity trust anchor is not the current root cert"),
        },
        Case {
            // Whitespace normalization, exactly as `out_of_sync_reason` arm 3 does:
            // trailing-newline drift between a Secret read and an in-memory PEM must
            // not be mistaken for a root swap.
            name: "trailing-whitespace difference is NOT a root change",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: "-----BEGIN CERTIFICATE-----\nROOT-A\n-----END CERTIFICATE-----\n\n  ",
            window: fresh,
            expected: None,
        },
        Case {
            name: "chains but inside the emergency window",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: ROOT_A,
            window: dying,
            expected: Some("server leaf within the emergency window"),
        },
        Case {
            name: "emergency boundary is inclusive",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: ROOT_A,
            window: at_emergency,
            expected: Some("server leaf within the emergency window"),
        },
        Case {
            name: "chains, not dying, but past the lifetime ratio",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: ROOT_A,
            window: half_life,
            expected: Some("server leaf past its lifetime ratio"),
        },
        Case {
            name: "lifetime-ratio boundary is inclusive",
            chains: true,
            identity_ca: ROOT_A,
            current_ca: ROOT_A,
            window: at_ratio,
            expected: Some("server leaf past its lifetime ratio"),
        },
    ];

    for c in &cases {
        assert_eq!(
            identity_resign_reason(c.chains, c.identity_ca, c.current_ca, c.window, ratio, emergency, now),
            c.expected,
            "case: {}",
            c.name
        );
    }
}

/// The reason string order matters: emergency must beat the lifetime ratio for
/// a leaf that is both (so the log says the more urgent thing).
#[test]
fn emergency_outranks_lifetime_ratio() {
    let now = datetime!(2026-06-15 12:00 UTC);
    // Old AND dying: past half-life and inside the emergency window.
    let window = (now - time::Duration::days(364), now + time::Duration::hours(1));
    assert_eq!(
        identity_resign_reason(true, ROOT_A, ROOT_A, window, 0.5, time::Duration::hours(24), now),
        Some("server leaf within the emergency window")
    );
}

/// The half-life rule is applied regardless of the configured policy: this
/// helper takes no policy at all (see the doc comment) — a `manual`-policy
/// deployment still keeps its own server identity current.
#[test]
fn ratio_zero_and_one_boundaries() {
    let now = datetime!(2026-06-15 12:00 UTC);
    let window = (now - time::Duration::days(1), now + time::Duration::days(364));
    let emergency = time::Duration::hours(24);
    // ratio 0.0 → trigger is notBefore, which is in the past → always due.
    assert_eq!(
        identity_resign_reason(true, ROOT_A, ROOT_A, window, 0.0, emergency, now),
        Some("server leaf past its lifetime ratio")
    );
    // ratio 1.0 → trigger is notAfter, still in the future → not due (the
    // emergency arm is the only net left, and it has not fired yet).
    assert_eq!(
        identity_resign_reason(true, ROOT_A, ROOT_A, window, 1.0, emergency, now),
        None
    );
}

#[tokio::test]
async fn ca_read_cannot_adopt_and_overwrite_a_concurrent_identity_publication() {
    use crate::services::conf_sync_ca::{build_secret, Ca, CA_SECRET};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    let _ = rustls::crypto::ring::default_provider().install_default();
    let ca_a = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let ca_b = Ca::generate_root(time::Duration::days(3650)).unwrap();
    let sans = vec!["localhost".to_string()];
    // Force a candidate even with the correct early A snapshot: the current
    // twelve-hour leaf is inside the default twenty-four-hour emergency window.
    let a = ca_a.sign_server(&sans, time::Duration::hours(12)).unwrap();
    let b = ca_b.sign_server(&sans, time::Duration::days(7)).unwrap();
    let handle = Arc::new(ServerIdentityHandle::new(
        ServerIdentity::from_pems(&a.cert_pem, &a.key_pem, ca_a.ca_cert_pem()).unwrap(),
    ));
    let replacement = ServerIdentity::from_pems(&b.cert_pem, &b.key_pem, ca_b.ca_cert_pem()).unwrap();
    let replacement_config = replacement.config.clone();
    let replacement = Arc::new(Mutex::new(Some(replacement)));
    let response = serde_json::to_vec(&build_secret(
        CA_SECRET,
        "test",
        BTreeMap::from([
            ("ca.crt".into(), ca_a.ca_cert_pem().as_bytes().to_vec()),
            ("ca.key".into(), ca_a.ca_key_pem().as_bytes().to_vec()),
        ]),
    ))
    .unwrap();
    let transport_handle = handle.clone();
    let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        assert_eq!(request.method(), http::Method::GET);
        assert_eq!(
            request.uri().path(),
            format!("/api/v1/namespaces/test/secrets/{CA_SECRET}")
        );
        // Publish B after this tick starts acquiring A but before its GET resolves.
        // The old producer captured B only after this response and overwrote it.
        transport_handle.store(replacement.lock().unwrap().take().unwrap());
        let response = response.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                http::Response::builder()
                    .status(200)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(kube::client::Body::from(response))
                    .unwrap(),
            )
        }
    });
    let api = Api::namespaced(kube::Client::new(transport, "test"), "test");
    sync_once_with_handle(&api, &sans, &ConfSyncRotationConfig::default(), &handle)
        .await
        .unwrap();
    let installed = handle.load();
    assert!(Arc::ptr_eq(&replacement_config, &installed.config));
    assert_eq!(installed.cert_pem, b.cert_pem);
    assert_eq!(installed.ca_cert_pem, ca_b.ca_cert_pem());
    assert_eq!(handle.trust_generation(), 1);
}

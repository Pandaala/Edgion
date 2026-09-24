use super::*;
use ::time::OffsetDateTime;

/// A minimal self-signed leaf cert PEM with an explicit validity window.
fn test_cert_pem(start_ts: i64, end_ts: i64) -> String {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    params.not_before = OffsetDateTime::from_unix_timestamp(start_ts).unwrap();
    params.not_after = OffsetDateTime::from_unix_timestamp(end_ts).unwrap();
    params.self_signed(&key).unwrap().pem()
}

const T0: i64 = 1_800_000_000; // arbitrary fixed epoch base for readable math
const DAY: i64 = 86_400;

#[test]
fn parse_leaf_validity_returns_the_signed_window() {
    let cert = test_cert_pem(T0, T0 + 30 * DAY);
    let v = parse_leaf_validity(&cert).unwrap();
    // rcgen truncates to whole seconds; the window must round-trip exactly.
    assert_eq!(v.not_before, T0);
    assert_eq!(v.not_after, T0 + 30 * DAY);
}

#[test]
fn parse_leaf_validity_rejects_garbage() {
    assert!(parse_leaf_validity("not a pem").is_err());
    assert!(parse_leaf_validity("-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n").is_err());
}

#[test]
fn remaining_seconds_counts_down_and_goes_negative_after_expiry() {
    let v = LeafValidity {
        not_before: T0,
        not_after: T0 + 10 * DAY,
    };
    assert_eq!(v.remaining_seconds(T0), 10 * DAY);
    assert_eq!(v.remaining_seconds(T0 + 10 * DAY + 5), -5);
}

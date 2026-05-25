//! Unit tests for `cert.rs` (see `super`).
use super::*;
use crate::types::constants::secret_keys::tls::{CERT, KEY};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn create_test_tls(cert_pem: Option<&str>, key_pem: Option<&str>, hosts: Vec<String>) -> EdgionTls {
    let mut data = BTreeMap::new();
    if let Some(cert) = cert_pem {
        data.insert(CERT.to_string(), ByteString(cert.as_bytes().to_vec()));
    }
    if let Some(key) = key_pem {
        data.insert(KEY.to_string(), ByteString(key.as_bytes().to_vec()));
    }

    EdgionTls {
        metadata: Default::default(),
        spec: crate::types::EdgionTlsSpec {
            parent_refs: None,
            hosts,
            secret_ref: crate::types::resources::gateway::SecretObjectReference {
                group: None,
                kind: None,
                name: "test-secret".to_string(),
                namespace: Some("default".to_string()),
            },
            client_auth: None,
            min_tls_version: None,
            ciphers: None,
            extend: None,
            secret: Some(Secret {
                data: Some(data),
                ..Default::default()
            }),
            resolved_listeners: None,
        },
        status: None,
    }
}

#[test]
fn test_missing_secret() {
    let mut tls = create_test_tls(None, None, vec!["example.com".to_string()]);
    tls.spec.secret = None;

    let result = validate_cert(&tls);
    assert!(!result.is_valid);
    assert_eq!(result.errors.len(), 1);
    assert!(matches!(result.errors[0], CertValidationError::SecretNotFound));
}

#[test]
fn test_missing_cert() {
    let tls = create_test_tls(None, Some("fake-key"), vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(!result.is_valid);
    assert!(matches!(result.errors[0], CertValidationError::CertNotFound));
}

#[test]
fn test_missing_key() {
    let tls = create_test_tls(Some("fake-cert"), None, vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(!result.is_valid);
    // Should have KeyNotFound and CertParseError (because "fake-cert" is not valid PEM)
    assert!(result
        .errors
        .iter()
        .any(|e| matches!(e, CertValidationError::KeyNotFound)));
}

#[test]
fn test_invalid_pem() {
    let tls = create_test_tls(
        Some("not-a-valid-pem"),
        Some("not-a-valid-key"),
        vec!["example.com".to_string()],
    );

    let result = validate_cert(&tls);
    assert!(!result.is_valid);
    assert!(result
        .errors
        .iter()
        .any(|e| matches!(e, CertValidationError::CertParseError(_))));
}

#[test]
fn test_is_host_covered() {
    let san_list = vec!["example.com".to_string(), "*.api.example.com".to_string()];

    // Exact match
    assert!(is_host_covered("example.com", &san_list));
    assert!(is_host_covered("EXAMPLE.COM", &san_list)); // Case insensitive

    // Wildcard match
    assert!(is_host_covered("v1.api.example.com", &san_list));
    assert!(is_host_covered("v2.api.example.com", &san_list));

    // No match
    assert!(!is_host_covered("other.com", &san_list));
    assert!(!is_host_covered("api.example.com", &san_list)); // Wildcard doesn't cover base
}

#[test]
fn test_is_valid_san_pattern() {
    // No wildcard: always valid
    assert!(is_valid_san_pattern("example.com"));
    assert!(is_valid_san_pattern("api.staging.example.com"));

    // Single leftmost-label wildcard: valid
    assert!(is_valid_san_pattern("*.example.com"));
    assert!(is_valid_san_pattern("*.staging.example.com"));

    // Multi-label wildcard: invalid (RFC 6125 §6.4.3 explicitly forbidden)
    assert!(!is_valid_san_pattern("*.*.example.com"));
    assert!(!is_valid_san_pattern("*.*.*.example.com"));

    // Non-leftmost / partial-label wildcard: invalid
    assert!(!is_valid_san_pattern("a.*.example.com"));
    assert!(!is_valid_san_pattern("foo*.example.com"));

    // Degenerate wildcards: invalid
    assert!(!is_valid_san_pattern("*."));
    assert!(!is_valid_san_pattern("*"));
}

#[test]
fn test_validate_cert_reports_invalid_san_pattern() {
    // A multi-star declared host must surface as InvalidSanPattern, not a generic
    // SanMismatch, so operators see the real problem (unsupported wildcard syntax).
    let (cert_pem, key_pem) = make_cert_with_sans(vec!["example.com"], vec![]);
    let tls = create_test_tls(
        Some(&cert_pem),
        Some(&key_pem),
        vec!["*.*.staging.example.com".to_string()],
    );

    let result = validate_cert(&tls);
    assert!(!result.is_valid, "multi-star host should be invalid");
    assert!(
        result.errors.iter().any(
            |e| matches!(e, CertValidationError::InvalidSanPattern { pattern } if pattern == "*.*.staging.example.com")
        ),
        "should report InvalidSanPattern, got: {:?}",
        result.errors
    );
    // The invalid pattern must NOT also produce a SanMismatch for the same host.
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
        "invalid pattern should not also report SanMismatch, got: {:?}",
        result.errors
    );
}

#[test]
fn test_validate_cert_reports_no_hosts_declared() {
    // An EdgionTls with empty spec.hosts must report NoHostsDeclared rather than
    // silently passing SAN validation (the empty-loop vacuous-pass guard).
    let (cert_pem, key_pem) = make_cert_with_sans(vec!["example.com"], vec![]);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec![]);

    let result = validate_cert(&tls);
    assert!(!result.is_valid, "empty hosts should be invalid");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::NoHostsDeclared)),
        "should report NoHostsDeclared, got: {:?}",
        result.errors
    );
    // Empty hosts must NOT spuriously also produce a SanMismatch.
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
        "empty hosts should not also report SanMismatch, got: {:?}",
        result.errors
    );
}

#[test]
fn test_validate_cert_single_wildcard_not_flagged() {
    // A valid single-label wildcard host that the cert covers must not be flagged.
    let (cert_pem, key_pem) = make_cert_with_sans(vec!["*.example.com"], vec![]);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["*.example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::InvalidSanPattern { .. })),
        "valid single-label wildcard should not be flagged, got: {:?}",
        result.errors
    );
}

#[test]
fn test_validate_mtls_config_mutual_no_ca() {
    use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};

    let config = ClientAuthConfig {
        mode: ClientAuthMode::Mutual,
        ca_secret_ref: None, // Missing CA Secret
        verify_depth: 1,
        ca_secret: None,
        allowed_sans: None,
        allowed_cns: None,
    };

    let mut errors = Vec::new();
    validate_mtls_config(&config, &mut errors);

    assert!(!errors.is_empty());
    assert!(errors
        .iter()
        .any(|e| matches!(e, CertValidationError::MtlsCaSecretRefRequired)));
}

#[test]
fn test_validate_mtls_config_verify_depth_out_of_range() {
    use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
    use crate::types::resources::gateway::SecretObjectReference;

    let config = ClientAuthConfig {
        mode: ClientAuthMode::Mutual,
        ca_secret_ref: Some(SecretObjectReference {
            name: "client-ca".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }),
        verify_depth: 10, // Out of range (max is 9)
        ca_secret: None,
        allowed_sans: None,
        allowed_cns: None,
    };

    let mut errors = Vec::new();
    validate_mtls_config(&config, &mut errors);

    assert!(!errors.is_empty());
    assert!(errors
        .iter()
        .any(|e| matches!(e, CertValidationError::MtlsVerifyDepthOutOfRange(10))));
}

#[test]
fn test_validate_mtls_config_invalid_san_pattern() {
    use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
    use crate::types::resources::gateway::SecretObjectReference;

    let config = ClientAuthConfig {
        mode: ClientAuthMode::Mutual,
        ca_secret_ref: Some(SecretObjectReference {
            name: "client-ca".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }),
        verify_depth: 1,
        ca_secret: None,
        allowed_sans: Some(vec!["".to_string(), "  ".to_string()]), // Empty patterns
        allowed_cns: None,
    };

    let mut errors = Vec::new();
    validate_mtls_config(&config, &mut errors);

    assert!(!errors.is_empty());
    assert!(errors
        .iter()
        .any(|e| matches!(e, CertValidationError::MtlsInvalidSanPattern(_))));
}

#[test]
fn test_validate_mtls_config_valid() {
    use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
    use crate::types::resources::gateway::SecretObjectReference;

    let config = ClientAuthConfig {
        mode: ClientAuthMode::Mutual,
        ca_secret_ref: Some(SecretObjectReference {
            name: "client-ca".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }),
        verify_depth: 2,
        ca_secret: None,
        allowed_sans: Some(vec!["client1.example.com".to_string()]),
        allowed_cns: Some(vec!["AdminClient".to_string()]),
    };

    let mut errors = Vec::new();
    validate_mtls_config(&config, &mut errors);

    assert!(errors.is_empty(), "Valid mTLS config should have no errors");
}

#[test]
fn test_validate_mtls_config_terminate_mode() {
    use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};

    let config = ClientAuthConfig {
        mode: ClientAuthMode::Terminate,
        ca_secret_ref: None, // CA not required for Terminate mode
        verify_depth: 1,
        ca_secret: None,
        allowed_sans: None,
        allowed_cns: None,
    };

    let mut errors = Vec::new();
    validate_mtls_config(&config, &mut errors);

    assert!(errors.is_empty(), "Terminate mode should not require CA Secret");
}

fn make_cert_with_validity(not_before_offset_days: i64, not_after_offset_days: i64) -> (String, String) {
    use rcgen::{CertificateParams, KeyPair};

    // Use absolute crate paths to avoid conflict with x509_parser::prelude::time module
    let now = ::time::OffsetDateTime::now_utc();
    let dur_before = ::time::Duration::days(not_before_offset_days);
    let dur_after = ::time::Duration::days(not_after_offset_days);

    let key_pair = KeyPair::generate().expect("rcgen key generation failed");
    let mut params = CertificateParams::new(vec!["example.com".to_string()]).expect("rcgen params failed");
    params.not_before = now + dur_before;
    params.not_after = now + dur_after;
    let cert = params.self_signed(&key_pair).expect("rcgen self-sign failed");
    (cert.pem(), key_pair.serialize_pem())
}

fn make_cert_with_sans(dns_names: Vec<&str>, ip_addrs: Vec<std::net::IpAddr>) -> (String, String) {
    use rcgen::{CertificateParams, KeyPair, SanType};

    let mut san_types: Vec<SanType> = dns_names
        .into_iter()
        .map(|name| SanType::DnsName(name.try_into().unwrap()))
        .collect();
    san_types.extend(ip_addrs.into_iter().map(SanType::IpAddress));

    let key_pair = KeyPair::generate().expect("key generation failed");
    let mut params = CertificateParams::new(vec![]).expect("params failed");
    params.subject_alt_names = san_types;
    let cert = params.self_signed(&key_pair).expect("self-sign failed");
    (cert.pem(), key_pair.serialize_pem())
}

fn make_cert_chain(
    leaf_not_before_days: i64,
    leaf_not_after_days: i64,
    intermediate_not_before_days: i64,
    intermediate_not_after_days: i64,
) -> (String, String) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

    let now = ::time::OffsetDateTime::now_utc();

    // Generate intermediate CA
    let ca_key = KeyPair::generate().expect("CA key gen failed");
    let mut ca_params = CertificateParams::new(vec!["Edgion Test CA".to_string()]).expect("CA params failed");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.not_before = now + ::time::Duration::days(intermediate_not_before_days);
    ca_params.not_after = now + ::time::Duration::days(intermediate_not_after_days);
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA self-sign failed");
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

    // Generate leaf certificate signed by CA
    let leaf_key = KeyPair::generate().expect("leaf key gen failed");
    let mut leaf_params = CertificateParams::new(vec!["example.com".to_string()]).expect("leaf params failed");
    leaf_params.not_before = now + ::time::Duration::days(leaf_not_before_days);
    leaf_params.not_after = now + ::time::Duration::days(leaf_not_after_days);
    let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf sign failed");

    // Chain: leaf + intermediate
    let chain_pem = format!("{}{}", leaf_cert.pem(), ca_cert.pem());
    (chain_pem, leaf_key.serialize_pem())
}

#[test]
fn test_expired_cert_is_invalid() {
    // Cert expired yesterday
    let (cert_pem, key_pem) = make_cert_with_validity(-30, -1);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(!result.is_valid, "expired cert should be invalid");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::CertExpired { .. })),
        "should report CertExpired, got: {:?}",
        result.errors
    );
}

#[test]
fn test_not_yet_valid_cert_is_invalid() {
    // Cert becomes valid tomorrow
    let (cert_pem, key_pem) = make_cert_with_validity(1, 365);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(!result.is_valid, "not-yet-valid cert should be invalid");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::CertExpired { .. })),
        "should report CertExpired, got: {:?}",
        result.errors
    );
}

#[test]
fn test_valid_cert_passes_expiry_check() {
    // Cert valid since yesterday, expires in one year
    let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    // Only assert that there is no CertExpired error
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::CertExpired { .. })),
        "valid cert should not report CertExpired, got: {:?}",
        result.errors
    );
}

#[test]
fn test_matching_key_cert_passes() {
    let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
    let result = check_key_matches_cert(&cert_pem, &key_pem);
    assert!(result.is_ok(), "matching cert/key should pass, got: {:?}", result);
}

#[test]
fn test_mismatched_key_cert_fails() {
    let (cert_a, _key_a) = make_cert_with_validity(-1, 365);
    let (_cert_b, key_b) = make_cert_with_validity(-1, 365);

    let result = check_key_matches_cert(&cert_a, &key_b);
    assert!(result.is_err(), "mismatched cert/key should fail");
    assert!(
        matches!(result, Err(CertValidationError::KeyMismatch(_))),
        "should return KeyMismatch, got: {:?}",
        result
    );
}

#[test]
fn test_validate_cert_rejects_mismatched_key() {
    let (cert_a, _key_a) = make_cert_with_validity(-1, 365);
    let (_cert_b, key_b) = make_cert_with_validity(-1, 365);

    let tls = create_test_tls(Some(&cert_a), Some(&key_b), vec!["example.com".to_string()]);
    let result = validate_cert(&tls);

    assert!(!result.is_valid, "mismatched cert/key should be invalid");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::KeyMismatch(_))),
        "should report KeyMismatch, got: {:?}",
        result.errors
    );
}

// --- IP SAN tests ---

#[test]
fn test_ip_san_ipv4_match() {
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let (cert_pem, key_pem) = make_cert_with_sans(vec![], vec![ip]);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["10.0.0.1".to_string()]);

    let result = validate_cert(&tls);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
        "IPv4 SAN should match, got: {:?}",
        result.errors
    );
}

#[test]
fn test_ip_san_ipv6_match() {
    let ip: std::net::IpAddr = "fd00::1".parse().unwrap();
    let (cert_pem, key_pem) = make_cert_with_sans(vec![], vec![ip]);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["fd00::1".to_string()]);

    let result = validate_cert(&tls);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
        "IPv6 SAN should match, got: {:?}",
        result.errors
    );
}

#[test]
fn test_mixed_dns_and_ip_san() {
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let (cert_pem, key_pem) = make_cert_with_sans(vec!["example.com"], vec![ip]);
    let tls = create_test_tls(
        Some(&cert_pem),
        Some(&key_pem),
        vec!["example.com".to_string(), "10.0.0.1".to_string()],
    );

    let result = validate_cert(&tls);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
        "mixed DNS+IP SAN should both match, got: {:?}",
        result.errors
    );
}

#[test]
fn test_ip_host_not_matched_by_dns_san() {
    let (cert_pem, key_pem) = make_cert_with_sans(vec!["example.com"], vec![]);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["10.0.0.1".to_string()]);

    let result = validate_cert(&tls);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
        "IP host should not match DNS SAN, got: {:?}",
        result.errors
    );
}

// --- Certificate chain tests ---

#[test]
fn test_cert_chain_valid() {
    let (chain_pem, key_pem) = make_cert_chain(-1, 365, -30, 3650);
    let tls = create_test_tls(Some(&chain_pem), Some(&key_pem), vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::CertParseError(_))),
        "valid chain should pass, got: {:?}",
        result.errors
    );
}

#[test]
fn test_cert_chain_expired_intermediate() {
    // Leaf valid, intermediate expired yesterday
    let (chain_pem, key_pem) = make_cert_chain(-1, 365, -3650, -1);
    let tls = create_test_tls(Some(&chain_pem), Some(&key_pem), vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(!result.is_valid, "expired intermediate should be invalid");
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::ChainCertExpired { .. })),
        "should report ChainCertExpired, got: {:?}",
        result.errors
    );
}

#[test]
fn test_single_cert_still_works() {
    // Single cert (no chain) should still work as before
    let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
    let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

    let result = validate_cert(&tls);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::CertParseError(_))),
        "single cert should still work, got: {:?}",
        result.errors
    );
}

#[test]
fn test_deprecated_min_tls_version_is_not_rejected() {
    use crate::types::resources::edgion_tls::TlsVersion;

    // RFC 8996 divergence: TLS 1.0/1.1 stay admissible as an operator opt-in
    // escape hatch. Setting them must emit a warn but must NOT invalidate the
    // resource. This test guards the "do not hard-reject" decision.
    let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
    let mut tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);
    tls.spec.min_tls_version = Some(TlsVersion::Tls10);

    let result = validate_cert(&tls);
    assert!(
        result.is_valid,
        "deprecated min_tls_version (TLS 1.0) must not be rejected, got: {:?}",
        result.errors
    );
}

#[test]
fn test_unimplemented_revocation_config_is_not_rejected() {
    use crate::types::resources::edgion_tls::{
        EdgionTlsExtend, OcspStaplingConfig, RevocationCheckConfig, RevocationMode,
    };

    // tls-07 divergence: revocationCheck/ocspStapling are reserved-but-unimplemented
    // schema. Setting them must emit a config-time warn but must NOT invalidate the
    // resource (hard-rejecting would take down a working TLS listener over a no-op
    // field). This test guards the "do not hard-reject" decision.
    let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
    let mut tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);
    tls.spec.extend = Some(EdgionTlsExtend {
        prefer_server_ciphers: false,
        ocsp_stapling: Some(OcspStaplingConfig {
            enabled: true,
            refresh_interval_seconds: None,
            fail_open: false,
        }),
        session_ticket: None,
        session_cache: None,
        revocation_check: Some(RevocationCheckConfig {
            mode: RevocationMode::Ocsp,
            fail_open: false,
        }),
        early_data: None,
    });

    let result = validate_cert(&tls);
    assert!(
        result.is_valid,
        "unimplemented revocationCheck/ocspStapling must not be rejected, got: {:?}",
        result.errors
    );
}

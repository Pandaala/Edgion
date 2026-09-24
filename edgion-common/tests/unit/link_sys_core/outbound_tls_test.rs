use super::*;
use edgion_resources::resources::backend_tls_policy::{
    BackendTLSPolicyCACertificateRef, SubjectAltName, SubjectAltNameType, WellKnownCACertificates,
};
use edgion_resources::resources::common::SecretObjectReference;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn cert_ref(kind: &str, name: &str) -> BackendTLSPolicyCACertificateRef {
    BackendTLSPolicyCACertificateRef {
        group: String::new(),
        kind: kind.into(),
        name: name.into(),
        namespace: None,
    }
}

fn ca_secret(pem: &[u8]) -> Secret {
    let mut data = BTreeMap::new();
    data.insert(tls_keys::CA_CERT.to_string(), ByteString(pem.to_vec()));
    Secret {
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    }
}

fn client_secret() -> Secret {
    let mut data = BTreeMap::new();
    data.insert(tls_keys::CERT.to_string(), ByteString(b"cert-pem".to_vec()));
    data.insert(tls_keys::KEY.to_string(), ByteString(b"key-pem".to_vec()));
    Secret {
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    }
}

fn secret_ref(name: &str) -> SecretObjectReference {
    SecretObjectReference {
        group: None,
        kind: None,
        name: name.to_string(),
        namespace: None,
    }
}

fn hostname_san(h: &str) -> SubjectAltName {
    SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some(h.to_string()),
        uri: None,
    }
}

#[test]
fn aggregate_verify_safest_wins() {
    let a = OutboundTlsConfig {
        verify: false,
        ..Default::default()
    };
    let b = OutboundTlsConfig::default(); // verify = true
    assert!(aggregate_verify(&[a.clone(), b]).verify);
    let c = OutboundTlsConfig {
        verify: false,
        ..Default::default()
    };
    assert!(!aggregate_verify(&[a, c]).verify);
}

#[test]
fn setting_global_default_reports_only_effective_changes() {
    let _guard = outbound_tls_test_guard();
    assert!(!set_outbound_global_default(OutboundTlsConfig::default()));
    assert!(set_outbound_global_default(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    }));
    assert!(!set_outbound_global_default(OutboundTlsConfig {
        verify: false,
        ..Default::default()
    }));
}

#[test]
fn verify_false_is_honored_when_provider_can_disable() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let local = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    let r = resolve_outbound_tls(&local, None, None, true, "Test").expect("verify=false must be honored");
    assert!(
        !r.verify,
        "resolved verify must be false when operator sets verify=false"
    );
}

#[test]
fn gate_rejects_skip_when_provider_cannot_disable() {
    let _guard = outbound_tls_test_guard();
    let local = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    assert!(resolve_outbound_tls(&local, None, None, false, "Redis").is_err());
}

#[test]
fn inherits_global_verify_true_when_local_none() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig {
        verify: true,
        ..Default::default()
    });
    let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
    assert!(r.verify);
}

#[test]
fn ca_refs_present_yields_multi_bundle_custom() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let s1 = ca_secret(b"-----BEGIN CERTIFICATE-----\nA\n-----END CERTIFICATE-----\n");
    let s2 = ca_secret(b"-----BEGIN CERTIFICATE-----\nB\n-----END CERTIFICATE-----\n");
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "a"), cert_ref("Secret", "b")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls(&local, Some(&[s1, s2]), None, true, "Test").unwrap();
    match r.ca {
        EffectiveCa::Custom(b) => assert_eq!(b.len(), 2),
        _ => panic!("expected multi-bundle Custom"),
    }
}

#[test]
fn well_known_system_yields_system() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            well_known_ca_certificates: Some(WellKnownCACertificates::System),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls(&local, None, None, true, "Test").unwrap();
    assert!(matches!(r.ca, EffectiveCa::System));
}

#[test]
fn unsupported_named_ca_never_falls_back_to_system() {
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            well_known_ca_certificates: Some(WellKnownCACertificates::Other("example.com/Custom".to_string())),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = resolve_outbound_tls_with_defaults(&local, None, None, true, &OutboundTlsConfig::default());
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsCaMissing)));
}

#[test]
fn both_absent_defaults_to_system_public_roots() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
    assert!(matches!(r.ca, EffectiveCa::System));
}

/// Fail-secure: a non-empty subjectAltNames policy overrides verify=false, so
/// the effective `verify` becomes true and the client mTLS identity is retained
/// (rather than dropped by the skip-verify path).
#[test]
fn san_policy_overrides_verify_false_and_keeps_client_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let local = OutboundTlsLocal {
        verify: Some(false),
        validation: Some(OutboundTlsValidation {
            subject_alt_names: Some(vec![hostname_san("idp.example.com")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls(&local, None, Some(&client_secret()), true, "Test").unwrap();
    assert!(r.verify, "SAN policy must force verify=true (fail-secure)");
    assert!(
        r.client_identity.is_some(),
        "client identity must be retained when SAN raises verify"
    );
}

/// verify=false with no SAN policy is a genuine skip-verify path: client
/// identity is dropped (M4).
#[test]
fn verify_false_without_san_drops_client_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let local = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    let r = resolve_outbound_tls(&local, None, Some(&client_secret()), true, "Test").unwrap();
    assert!(!r.verify);
    assert!(
        r.client_identity.is_none(),
        "skip-verify path must drop client identity"
    );
}

/// A local validation block declaring caCertificateRefs but with no per-call
/// resolved bytes must fail with OutboundTlsCaMissing — it must NOT silently
/// fall back to the global resolved CA (which belongs to the global refs).
#[test]
fn local_ca_refs_without_resolved_bytes_errors_no_global_fallback() {
    let _guard = outbound_tls_test_guard();
    // Global has its own resolved CA that must NOT be borrowed by the local block.
    set_outbound_global_default(OutboundTlsConfig {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "global-ca")]),
            ..Default::default()
        }),
        resolved_ca_certificates: Some(vec![ca_secret(
            b"-----BEGIN CERTIFICATE-----\nGLOBAL\n-----END CERTIFICATE-----\n",
        )]),
        ..Default::default()
    });
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "local-ca")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let r = resolve_outbound_tls(&local, None, None, true, "Test");
    assert!(
        matches!(r, Err(EdgionErrorCode::OutboundTlsCaMissing)),
        "local CA refs with no resolved bytes must error, got: {:?}",
        r.map(|_| "Ok(resolved)")
    );
}

#[test]
fn partially_resolved_ca_refs_fail_closed() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let local = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "ca-a"), cert_ref("Secret", "ca-b")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let only_one_secret = [ca_secret(
        b"-----BEGIN CERTIFICATE-----\nA\n-----END CERTIFICATE-----\n",
    )];

    let result = resolve_outbound_tls(&local, Some(&only_one_secret), None, true, "Test");
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsCaMissing)));
}

/// When the local block declares no validation, the global validation block
/// wins and its Controller-shipped resolved CA bundles are used.
#[test]
fn inherits_global_ca_when_local_validation_absent() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![cert_ref("Secret", "global-ca")]),
            ..Default::default()
        }),
        resolved_ca_certificates: Some(vec![ca_secret(
            b"-----BEGIN CERTIFICATE-----\nGLOBAL\n-----END CERTIFICATE-----\n",
        )]),
        ..Default::default()
    });
    let r = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test").unwrap();
    match r.ca {
        EffectiveCa::Custom(b) => assert_eq!(b.len(), 1),
        _ => panic!("expected global resolved CA to be inherited as Custom"),
    }
}

#[test]
fn local_client_ref_without_resolved_secret_fails_closed() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig::default());
    let local = OutboundTlsLocal {
        client_certificate_ref: Some(secret_ref("local-client")),
        ..Default::default()
    };

    let result = resolve_outbound_tls(&local, None, None, true, "Test");
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
}

#[test]
fn global_client_ref_without_resolved_secret_fails_closed() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig {
        client_certificate_ref: Some(secret_ref("global-client")),
        ..Default::default()
    });

    let result = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test");
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
    set_outbound_global_default(OutboundTlsConfig::default());
}

#[test]
fn explicit_defaults_do_not_inherit_process_client_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig {
        client_certificate_ref: Some(secret_ref("global-client")),
        resolved_client_certificate: Some(client_secret()),
        ..Default::default()
    });
    let resolved = resolve_outbound_tls_with_defaults(
        &OutboundTlsLocal::default(),
        None,
        None,
        false,
        &OutboundTlsConfig::default(),
    )
    .expect("explicit defaults must resolve independently");
    assert!(resolved.verify);
    assert!(matches!(resolved.ca, EffectiveCa::System));
    assert!(resolved.client_identity.is_none());
}

#[test]
fn global_client_identity_is_inherited_when_local_ref_is_absent() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig {
        client_certificate_ref: Some(secret_ref("global-client")),
        resolved_client_certificate: Some(client_secret()),
        ..Default::default()
    });

    let resolved = resolve_outbound_tls(&OutboundTlsLocal::default(), None, None, true, "Test")
        .expect("global client identity must resolve");
    assert!(resolved.client_identity.is_some());
    set_outbound_global_default(OutboundTlsConfig::default());
}

#[test]
fn local_client_ref_does_not_borrow_global_resolved_identity() {
    let _guard = outbound_tls_test_guard();
    set_outbound_global_default(OutboundTlsConfig {
        client_certificate_ref: Some(secret_ref("global-client")),
        resolved_client_certificate: Some(client_secret()),
        ..Default::default()
    });
    let local = OutboundTlsLocal {
        client_certificate_ref: Some(secret_ref("local-client")),
        ..Default::default()
    };

    let result = resolve_outbound_tls(&local, None, None, true, "Test");
    assert!(matches!(result, Err(EdgionErrorCode::OutboundTlsClientCertInvalid)));
    set_outbound_global_default(OutboundTlsConfig::default());
}

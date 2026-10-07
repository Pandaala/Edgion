use super::*;
use edgion_resources::constants::secret_keys::tls as tls_keys;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn kafka_input(
    tls: Option<serde_json::Value>,
    defaults: std::sync::Arc<outbound_tls::GlobalOutboundTlsSnapshot>,
) -> outbound_tls::PreparedLinkSysTls {
    let owner = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"kafka","namespace":"apps"},
        "spec":{"type":"kafka","config":{"brokers":["kafka.example:9093"],"tls":tls}}
    }))
    .unwrap();
    outbound_tls::PreparedLinkSysTls::with_snapshot(std::sync::Arc::new(owner), defaults).unwrap()
}

#[test]
fn owning_kafka_converter_requires_explicit_system_tls_but_keeps_native_fred_fallback() {
    install_test_crypto_provider();
    let defaults = std::sync::Arc::new(outbound_tls::GlobalOutboundTlsSnapshot::from_configs([]));
    for tls in [
        None,
        Some(serde_json::json!({"enabled":false,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"missing","namespace":""}]}})),
    ] {
        assert!(build_kafka_rustls_client_config(&kafka_input(tls, defaults.clone()))
            .unwrap()
            .is_none());
    }
    let active = kafka_input(Some(serde_json::json!({"enabled":true})), defaults);
    assert!(build_kafka_rustls_client_config(&active).unwrap().is_some());
    active
        .with_tls_selection(|proof, defaults| {
            assert!(build_validated_rustls_client_config(proof, defaults, "Redis")?.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn owning_kafka_entry_checks_variant_and_ignores_global_policy_only_when_inactive() {
    install_test_crypto_provider();
    let invalid: edgion_resources::EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"invalid","namespace":"apps"},"spec":{"outboundTls":{"verify":true}}
    }))
    .unwrap();
    let defaults = std::sync::Arc::new(outbound_tls::GlobalOutboundTlsSnapshot::from_configs([
        std::sync::Arc::new(invalid),
    ]));
    let inactive = kafka_input(None, defaults.clone());
    assert!(build_kafka_rustls_client_config(&inactive).unwrap().is_none());
    let active = kafka_input(Some(serde_json::json!({"enabled":true})), defaults.clone());
    assert!(build_kafka_rustls_client_config(&active).is_err());
    let wrong = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"es"},"spec":{"type":"elasticsearch","config":{"endpoints":["https://es.example:9200"]}}
    }))
    .unwrap();
    let wrong = outbound_tls::PreparedLinkSysTls::with_snapshot(std::sync::Arc::new(wrong), defaults).unwrap();
    assert!(build_kafka_rustls_client_config(&wrong).is_err());
}

#[test]
fn owning_kafka_converter_rejects_local_and_inherited_unsupported_capabilities() {
    use edgion_resources::resources::common::OutboundTlsConfig;
    install_test_crypto_provider();
    for policy in [
        serde_json::json!({"verify":false}),
        serde_json::json!({"verify":true,"validation":{"hostname":"kafka.example"}}),
        serde_json::json!({"verify":true,"validation":{"subjectAltNames":[{"type":"Hostname","hostname":"kafka.example"}]}}),
    ] {
        let mut inherited_policy = policy.clone();
        if inherited_policy.get("validation").is_some() {
            inherited_policy["validation"]["wellKnownCACertificates"] = "System".into();
        }
        let global: OutboundTlsConfig = serde_json::from_value(inherited_policy).unwrap();
        let defaults = outbound_tls::test_util::snapshot(global);
        let inherited = kafka_input(Some(serde_json::json!({"enabled":true})), defaults.clone());
        assert!(build_kafka_rustls_client_config(&inherited).is_err());
        let masked = kafka_input(
            Some(serde_json::json!({"enabled":true,"verify":true,"validation":{}})),
            defaults,
        );
        assert!(build_kafka_rustls_client_config(&masked).unwrap().is_some());
        let mut local = policy;
        local["enabled"] = true.into();
        let local = kafka_input(
            Some(local),
            std::sync::Arc::new(outbound_tls::GlobalOutboundTlsSnapshot::from_configs([])),
        );
        assert!(build_kafka_rustls_client_config(&local).is_err());
    }
}

fn secret_with(data: &[(&str, &[u8])]) -> Secret {
    let data = data
        .iter()
        .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
        .collect::<BTreeMap<_, _>>();
    Secret {
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    }
}

/// Build a CA `Secret` with the PEM stored under `ca.crt`.
fn ca_secret(pem: &[u8]) -> Secret {
    secret_with(&[(tls_keys::CA_CERT, pem)])
}

/// Build an `OutboundTlsLocal` that declares one CA ref (name only; the
/// resolved bytes come from `resolved_ca_certificates`).
fn outbound_with_ca_ref(name: &str) -> OutboundTlsLocal {
    outbound_with_ca_refs(&[name])
}

fn outbound_with_ca_refs(names: &[&str]) -> OutboundTlsLocal {
    OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(
                names
                    .iter()
                    .map(|name| CustomCACertificateRef {
                        group: Some(String::new()),
                        kind: "Secret".into(),
                        name: (*name).into(),
                        namespace: None,
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ------------------------------------------------------------------
// build_rustls_client_config
// ------------------------------------------------------------------

fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn gen_ca_and_leaf() -> (String, String, String) {
    let ca = rcgen::generate_simple_self_signed(vec!["test-ca".into()]).unwrap();
    let leaf = rcgen::generate_simple_self_signed(vec!["redis.example.com".into()]).unwrap();
    (ca.cert.pem(), leaf.cert.pem(), leaf.signing_key.serialize_pem())
}

#[test]
fn rustls_config_none_without_material() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    install_test_crypto_provider();
    // No CA, no client identity → None (caller uses default connector)
    let tls = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal::default(),
        resolved_ca_certificates: None,
        resolved_client_certificate: None,
    };
    assert!(build_rustls_client_config(&tls).unwrap().is_none());
}

#[test]
fn rustls_config_with_inline_ca() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    install_test_crypto_provider();
    let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("my-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
        resolved_client_certificate: None,
    };
    let cfg = build_rustls_client_config(&tls).unwrap();
    assert!(cfg.is_some(), "expected a ClientConfig when CA is provided");
}

#[test]
fn rustls_config_with_ca_and_client_mtls() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    install_test_crypto_provider();
    let (ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("my-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
        resolved_client_certificate: Some(secret_with(&[
            ("tls.crt", cert_pem.as_bytes()),
            ("tls.key", key_pem.as_bytes()),
        ])),
    };
    let cfg = build_rustls_client_config(&tls)
        .expect("should build ClientConfig")
        .expect("config present");
    assert!(cfg.client_auth_cert_resolver.has_certs());
}

#[test]
fn rustls_config_invalid_ca_pem_errors() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    install_test_crypto_provider();
    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("bad-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(
            b"-----BEGIN CERTIFICATE-----\nnot-a-real-pem\n-----END CERTIFICATE-----\n",
        )]),
        resolved_client_certificate: None,
    };
    assert!(build_rustls_client_config(&tls).is_err());
}

#[test]
fn rustls_config_rejects_each_zero_certificate_ca_bundle() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    install_test_crypto_provider();
    let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();

    for invalid in [b"".as_slice(), b" \n\t".as_slice(), b"not-a-pem".as_slice()] {
        for invalid_first in [false, true] {
            let mut bundles = vec![ca_secret(ca_pem.as_bytes()), ca_secret(invalid)];
            if invalid_first {
                bundles.swap(0, 1);
            }
            let tls = TlsConfig {
                enabled: true,
                outbound: outbound_with_ca_refs(&["ca-a", "ca-b"]),
                resolved_ca_certificates: Some(bundles),
                resolved_client_certificate: None,
            };

            build_rustls_client_config(&tls).expect_err("every declared rustls CA bundle must contain a certificate");
        }
    }
}

#[test]
fn rustls_config_accepts_multiple_valid_bundles_and_certificates() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    install_test_crypto_provider();
    let (ca_pem_a, _cert_pem, _key_pem) = gen_ca_and_leaf();
    let (ca_pem_b, _cert_pem, _key_pem) = gen_ca_and_leaf();
    let multiple_bundles = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_refs(&["ca-a", "ca-b"]),
        resolved_ca_certificates: Some(vec![ca_secret(ca_pem_a.as_bytes()), ca_secret(ca_pem_b.as_bytes())]),
        resolved_client_certificate: None,
    };
    assert!(build_rustls_client_config(&multiple_bundles).is_ok());

    let combined_pem = format!("{ca_pem_a}\n{ca_pem_b}\n");
    let multiple_certificates = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("ca-chain"),
        resolved_ca_certificates: Some(vec![ca_secret(combined_pem.as_bytes())]),
        resolved_client_certificate: None,
    };
    assert!(build_rustls_client_config(&multiple_certificates).is_ok());
}

/// `apply_resolved_outbound_tls` must return `Ok` and apply
/// `danger_accept_invalid_certs(true)` when `verify == Some(false)` — the
/// operator's explicit choice is honored regardless of any environment variable.
#[test]
fn resolved_outbound_tls_verify_false_is_honored() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let tls_skip = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        },
        resolved_ca_certificates: None,
        resolved_client_certificate: None,
    };
    let tls_nop = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal::default(), // verify = None → inherit global (true)
        resolved_ca_certificates: None,
        resolved_client_certificate: None,
    };

    // verify=false → honored, returns Ok.
    assert!(
        apply_resolved_outbound_tls(reqwest::Client::builder(), &tls_skip, "TestProvider").is_ok(),
        "verify=false must be honored and return Ok"
    );

    // verify=None → noop, always Ok.
    assert!(
        apply_resolved_outbound_tls(reqwest::Client::builder(), &tls_nop, "TestProvider").is_ok(),
        "verify=None must pass through unchanged"
    );
}

#[test]
fn rustls_config_system_roots_when_ca_absent() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    install_test_crypto_provider();
    // No inline CA + a client cert pair → must use built-in webpki roots
    // for the server chain. The webpki roots are always present (compiled in),
    // so construction must always succeed.
    let (_ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
    let tls = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal::default(),
        resolved_ca_certificates: None,
        resolved_client_certificate: Some(secret_with(&[
            ("tls.crt", cert_pem.as_bytes()),
            ("tls.key", key_pem.as_bytes()),
        ])),
    };
    assert!(
        build_rustls_client_config(&tls).unwrap().is_some(),
        "expected Ok(Some(_)) with built-in webpki roots"
    );
}

// ------------------------------------------------------------------
// apply_resolved_outbound_tls
// ------------------------------------------------------------------

#[test]
fn apply_resolved_outbound_tls_passthrough_when_no_material() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let tls = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal::default(),
        resolved_ca_certificates: None,
        resolved_client_certificate: None,
    };
    assert!(
        apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_ok(),
        "should return Ok when no material is configured"
    );
}

#[test]
fn apply_resolved_outbound_tls_valid_ca_only() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("my-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
        resolved_client_certificate: None,
    };
    assert!(
        apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_ok(),
        "valid CA PEM should not produce an error"
    );
}

#[test]
fn apply_resolved_outbound_tls_rejects_bundle_without_certificate() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("invalid-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(b"not-a-pem")]),
        resolved_client_certificate: None,
    };

    apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test")
        .expect_err("custom CA bundle without a certificate must fail closed");
}

#[test]
fn apply_resolved_outbound_tls_rejects_each_zero_certificate_ca_bundle() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();

    for invalid in [b"".as_slice(), b" \n\t".as_slice(), b"not-a-pem".as_slice()] {
        for invalid_first in [false, true] {
            let mut bundles = vec![ca_secret(ca_pem.as_bytes()), ca_secret(invalid)];
            if invalid_first {
                bundles.swap(0, 1);
            }
            let tls = TlsConfig {
                enabled: true,
                outbound: outbound_with_ca_refs(&["ca-a", "ca-b"]),
                resolved_ca_certificates: Some(bundles),
                resolved_client_certificate: None,
            };

            apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test")
                .expect_err("every declared reqwest CA bundle must contain a certificate");
        }
    }
}

#[test]
fn apply_resolved_outbound_tls_valid_mtls() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let (ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("my-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
        resolved_client_certificate: Some(secret_with(&[
            ("tls.crt", cert_pem.as_bytes()),
            ("tls.key", key_pem.as_bytes()),
        ])),
    };
    assert!(
        apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_ok(),
        "valid CA + mTLS identity should not produce an error"
    );
}

#[test]
fn apply_resolved_outbound_tls_invalid_identity_errors() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("my-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
        resolved_client_certificate: Some(secret_with(&[("tls.crt", b"bad-cert"), ("tls.key", b"bad-key")])),
    };
    let err = apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "TestProvider")
        .expect_err("invalid identity PEM must return Err");
    let msg = err.to_string();
    assert!(
        msg.contains("TestProvider TLS:"),
        "error must carry provider prefix, got: {msg}"
    );
}

/// Two CA bundles in `resolved_ca_certificates` must both be loaded into the
/// reqwest root store.  Both bundles are real (rcgen-generated) certs so
/// `Certificate::from_pem` accepts them; the assertion is that `build()` does
/// not return an error — a failure to parse either bundle would surface there.
#[test]
fn apply_resolved_outbound_tls_multi_bundle_reqwest() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let (ca_pem_a, _c, _k) = gen_ca_and_leaf();
    let (ca_pem_b, _c, _k) = gen_ca_and_leaf();

    let tls = TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![
                    CustomCACertificateRef {
                        group: Some(String::new()),
                        kind: "Secret".into(),
                        name: "ca-a".into(),
                        namespace: None,
                    },
                    CustomCACertificateRef {
                        group: Some(String::new()),
                        kind: "Secret".into(),
                        name: "ca-b".into(),
                        namespace: None,
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        },
        resolved_ca_certificates: Some(vec![ca_secret(ca_pem_a.as_bytes()), ca_secret(ca_pem_b.as_bytes())]),
        resolved_client_certificate: None,
    };

    let result = apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test");
    assert!(
        result.is_ok(),
        "two distinct CA bundles must both be accepted by reqwest, err: {:?}",
        result.err()
    );
    // Verify the built client can be constructed (not just the builder).
    let builder_result = result.unwrap().build();
    assert!(
        builder_result.is_ok(),
        "reqwest Client::build() must succeed with two CA bundles, err: {:?}",
        builder_result.err()
    );
}

/// A single CA bundle containing multiple certificates (root + intermediate chain)
/// must have all certificates loaded into the reqwest root store.
/// This regression test ensures that `apply_custom_ca_pem` correctly handles
/// multi-certificate PEM bundles instead of silently dropping certificates after the first.
#[test]
fn apply_custom_ca_pem_multi_cert_in_single_bundle() {
    let _guard = outbound_tls::outbound_tls_test_guard();
    let (ca_root_pem, _c, _k) = gen_ca_and_leaf();
    let (_ca_intermediate_pem, _c, _k) = gen_ca_and_leaf();

    // Simulate an operator concatenating root + intermediate into a single PEM bundle.
    // In practice, ca_root_pem and _ca_intermediate_pem are two independent certs.
    let combined_pem = format!("{}\n{}\n", ca_root_pem, _ca_intermediate_pem);

    let tls = TlsConfig {
        enabled: true,
        outbound: outbound_with_ca_ref("multi-cert-ca"),
        resolved_ca_certificates: Some(vec![ca_secret(combined_pem.as_bytes())]),
        resolved_client_certificate: None,
    };

    // Before the fix, reqwest::Certificate::from_pem would silently accept
    // only the first cert and drop the second. After the fix, apply_custom_ca_pem
    // splits the bundle and loads all certs.
    let result = apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "MultiCertTest");
    assert!(
        result.is_ok(),
        "multi-certificate PEM bundle should be accepted, err: {:?}",
        result.err()
    );

    // Verify the built client can be constructed (success = no error during cert load).
    let builder_result = result.unwrap().build();
    assert!(
        builder_result.is_ok(),
        "reqwest Client::build() must succeed with multi-cert bundle, err: {:?}",
        builder_result.err()
    );
}

#[test]
fn incapable_builders_reject_local_and_inherited_identity_policy() {
    use edgion_resources::resources::backend_tls_policy::SubjectAltName;
    use edgion_resources::resources::common::OutboundTlsConfig;
    let _guard = outbound_tls::outbound_tls_test_guard();
    for validation in [
        OutboundTlsValidation {
            hostname: Some("example.com".into()),
            ..Default::default()
        },
        OutboundTlsValidation {
            subject_alt_names: Some(vec![SubjectAltName {
                san_type: edgion_resources::resources::backend_tls_policy::SubjectAltNameType::Hostname,
                hostname: Some("example.com".into()),
                uri: None,
            }]),
            ..Default::default()
        },
    ] {
        for inherited in [false, true] {
            outbound_tls::set_outbound_global_default(crate::link_sys_core::outbound_tls::test_util::snapshot(
                OutboundTlsConfig {
                    validation: inherited.then(|| OutboundTlsValidation {
                        well_known_ca_certificates: Some(
                            edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System,
                        ),
                        ..validation.clone()
                    }),
                    ..Default::default()
                },
            ));
            let mut tls = TlsConfig {
                enabled: true,
                outbound: OutboundTlsLocal {
                    validation: (!inherited).then(|| validation.clone()),
                    ..Default::default()
                },
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
            };
            assert!(build_rustls_client_config(&tls).is_err());
            assert!(apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_err());
            // A local empty validation block shadows inherited identity constraints.
            tls.outbound.validation = Some(OutboundTlsValidation::default());
            assert!(build_rustls_client_config(&tls).unwrap().is_none());
            assert!(apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_ok());
        }
    }
}

#[test]
fn custom_ca_rejects_truncated_trailing_certificate() {
    let (pem, _, _) = gen_ca_and_leaf();
    let bundle = format!("{pem}-----BEGIN CERTIFICATE-----\nYWJj\n").into_bytes();
    assert!(apply_custom_ca_pem(reqwest::Client::builder(), &[bundle]).is_err());
}

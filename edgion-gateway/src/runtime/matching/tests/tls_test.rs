//! Unit tests for `tls.rs` (see `super`).
use super::*;
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT as TLS_CERT, KEY as TLS_KEY};
use edgion_resources::resources::edgion_tls::{GatewayAttachmentProof, ResolvedListenerAttachment};
use edgion_resources::resources::gateway::{
    FrontendCACertificateRef, FrontendCASourceIdentity, FrontendCASourceKind, FrontendTLSValidation,
    FrontendValidationMode, GatewaySpec, GatewayTLSConfig, GatewayTlsMode, Listener, ResolvedFrontendCAReference,
    ResolvedServingCertificateReference, ServingCertificateSourceIdentity,
};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Helper: build a ListenerKey for test gateways (no addresses -> 0.0.0.0).
fn test_listener_key(port: u16) -> ListenerKey {
    ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port), "HTTPS")
}

fn create_test_gateway(name: &str, namespace: &str, listeners: Vec<Listener>) -> Gateway {
    crate::test_current_status::processed(Gateway {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        spec: GatewaySpec {
            current_status: None,
            gateway_class_name: "test-class".to_string(),
            listeners: Some(listeners),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: Default::default(),
            resolved_attachment_proof: None,
        },
        status: None,
    })
}

fn create_test_listener(name: &str, hostname: Option<&str>, tls: Option<GatewayTLSConfig>) -> Listener {
    create_test_listener_with_port(name, 443, hostname, tls)
}

fn create_test_listener_with_port(
    name: &str,
    port: i32,
    hostname: Option<&str>,
    tls: Option<GatewayTLSConfig>,
) -> Listener {
    Listener {
        name: name.to_string(),
        hostname: hostname.map(|s| s.to_string()),
        port,
        protocol: "HTTPS".to_string(),
        tls,
        allowed_routes: None,
    }
}

fn make_tls_config(cert_name: &str, cert_ns: Option<&str>) -> GatewayTLSConfig {
    GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: cert_name.to_string(),
            namespace: cert_ns.map(|s| s.to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    }
}

fn frontend_ref(name: &str) -> FrontendCACertificateRef {
    FrontendCACertificateRef {
        group: String::new(),
        kind: "ConfigMap".to_string(),
        name: name.to_string(),
        namespace: None,
    }
}

fn ca_secret(pem: &[u8]) -> Secret {
    Secret {
        data: Some(BTreeMap::from([(CA_CERT.to_string(), ByteString(pem.to_vec()))])),
        ..Default::default()
    }
}

fn valid_ca_secret() -> Secret {
    ca_secret(&valid_ca_pem())
}

fn valid_ca_pem() -> Vec<u8> {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["client.example.com".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.self_signed(&key).unwrap().pem().into_bytes()
}

fn valid_server_secret() -> Secret {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["server.example.com".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap().pem();
    Secret {
        metadata: ObjectMeta {
            name: Some("expected".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            (TLS_CERT.to_string(), ByteString(cert.into_bytes())),
            (TLS_KEY.to_string(), ByteString(key.serialize_pem().into_bytes())),
        ])),
        ..Default::default()
    }
}

fn with_frontend(
    mut tls: GatewayTLSConfig,
    mode: Option<FrontendValidationMode>,
    refs: Vec<FrontendCACertificateRef>,
    resolved: Option<Vec<Secret>>,
) -> GatewayTLSConfig {
    // Frontend outcome tests use the EdgionTls certificate-provider shape so
    // serving-certificate resolution is not an unrelated variable.
    tls.certificate_refs = None;
    tls.resolved_certificate_refs = None;
    tls.frontend_matcher_eligibility = Some(FrontendMatcherEligibility::RequireExactEdgionTlsCertificate);
    tls.frontend_validation = Some(FrontendTLSValidation {
        ca_certificate_refs: refs,
        mode,
    });
    tls.resolved_frontend_ca_refs = resolved.map(|secrets| {
        secrets
            .into_iter()
            .enumerate()
            .map(|(index, secret)| ResolvedFrontendCAReference {
                index: index as u8,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::ConfigMap,
                    namespace: "default".to_string(),
                    name: tls.frontend_validation.as_ref().unwrap().ca_certificate_refs[index]
                        .name
                        .clone(),
                },
                ca_certificate: secret.data.and_then(|mut data| data.remove(CA_CERT)),
                error: None,
            })
            .collect()
    });
    tls
}

#[test]
fn strict_frontend_outcome_requires_complete_valid_ca_set() {
    let matcher = GatewayTlsMatcher::new();
    let cases = [
        (
            vec![frontend_ref("one")],
            None,
            FrontendMtlsOutcome::StrictReject {
                reason: "ca unresolved",
            },
        ),
        (
            vec![frontend_ref("one"), frontend_ref("two")],
            Some(vec![valid_ca_secret()]),
            FrontendMtlsOutcome::StrictReject {
                reason: "ca set incomplete",
            },
        ),
        (
            vec![frontend_ref("one")],
            Some(vec![ca_secret(b"garbage")]),
            FrontendMtlsOutcome::StrictReject {
                reason: "ca pem invalid",
            },
        ),
        (
            vec![frontend_ref("one")],
            Some(vec![valid_ca_secret()]),
            FrontendMtlsOutcome::StrictEffective,
        ),
    ];
    for (refs, resolved, expected) in cases {
        let tls = with_frontend(make_tls_config("server", None), None, refs, resolved);
        let gateway = create_test_gateway(
            "gw",
            "default",
            vec![create_test_listener("https", Some("strict.example.com"), Some(tls))],
        );
        matcher.rebuild_from_gateways(&[gateway]);
        assert_eq!(
            matcher
                .match_sni(&test_listener_key(443), "strict.example.com")
                .unwrap()
                .frontend_mtls,
            expected
        );
    }
}

#[test]
fn strict_frontend_deduplicates_ca_certificates_across_refs() {
    let matcher = GatewayTlsMatcher::new();
    let ca = valid_ca_secret();
    let tls = with_frontend(
        make_tls_config("server", None),
        None,
        vec![frontend_ref("one"), frontend_ref("two")],
        Some(vec![ca.clone(), ca]),
    );
    matcher.rebuild_from_gateways(&[create_test_gateway(
        "gw",
        "default",
        vec![create_test_listener("https", Some("dedupe.example.com"), Some(tls))],
    )]);
    assert_eq!(
        matcher
            .match_sni(&test_listener_key(443), "dedupe.example.com")
            .unwrap()
            .frontend_mtls,
        FrontendMtlsOutcome::StrictEffective
    );
}

#[test]
fn frontend_accepts_v16_ca_reference_count_in_strict_and_fallback_modes() {
    let matcher = GatewayTlsMatcher::new();
    for count in [9, 16] {
        let refs = (0..count)
            .map(|index| frontend_ref(&format!("ca-{index}")))
            .collect::<Vec<_>>();
        for (mode, expected) in [
            (None, FrontendMtlsOutcome::StrictEffective),
            (
                Some(FrontendValidationMode::AllowInsecureFallback),
                FrontendMtlsOutcome::InsecureFallback,
            ),
        ] {
            let resolved = (0..count).map(|_| valid_ca_secret()).collect();
            let tls = with_frontend(make_tls_config("server", None), mode, refs.clone(), Some(resolved));
            let gateway = create_test_gateway(
                "gw",
                "default",
                vec![create_test_listener("https", Some("limit.example.com"), Some(tls))],
            );
            matcher.rebuild_from_gateways(&[gateway]);
            assert_eq!(
                matcher
                    .match_sni(&test_listener_key(443), "limit.example.com")
                    .unwrap()
                    .frontend_mtls,
                expected,
            );
        }
    }
}

#[test]
fn strict_frontend_enforces_aggregate_byte_and_certificate_limits() {
    let matcher = GatewayTlsMatcher::new();
    let mut padded_pem = valid_ca_pem();
    padded_pem.resize(edgion_common::tls_validation::MAX_CA_BUNDLE_BYTES_PER_REF, b'\n');
    let refs = (0..5)
        .map(|index| frontend_ref(&format!("bytes-{index}")))
        .collect::<Vec<_>>();
    let tls = with_frontend(
        make_tls_config("server", None),
        None,
        refs,
        Some((0..5).map(|_| ca_secret(&padded_pem)).collect()),
    );
    matcher.rebuild_from_gateways(&[create_test_gateway(
        "gw",
        "default",
        vec![create_test_listener("https", Some("bytes.example.com"), Some(tls))],
    )]);
    assert_eq!(
        matcher
            .match_sni(&test_listener_key(443), "bytes.example.com")
            .unwrap()
            .frontend_mtls,
        FrontendMtlsOutcome::StrictReject {
            reason: "ca aggregate bytes exceeded"
        }
    );

    let refs = (0..8)
        .map(|index| frontend_ref(&format!("count-{index}")))
        .collect::<Vec<_>>();
    let resolved = (0..8)
        .map(|_| {
            let bundle = (0..129).flat_map(|_| valid_ca_pem()).collect::<Vec<_>>();
            ca_secret(&bundle)
        })
        .collect();
    let tls = with_frontend(make_tls_config("server", None), None, refs, Some(resolved));
    matcher.rebuild_from_gateways(&[create_test_gateway(
        "gw",
        "default",
        vec![create_test_listener("https", Some("count.example.com"), Some(tls))],
    )]);
    assert_eq!(
        matcher
            .match_sni(&test_listener_key(443), "count.example.com")
            .unwrap()
            .frontend_mtls,
        FrontendMtlsOutcome::StrictReject {
            reason: "ca aggregate certificate count exceeded"
        }
    );
}

#[test]
fn insecure_fallback_replaces_prior_strict_ca_generation() {
    let matcher = GatewayTlsMatcher::new();
    let strict = with_frontend(
        make_tls_config("server", None),
        None,
        vec![frontend_ref("one")],
        Some(vec![valid_ca_secret()]),
    );
    matcher.rebuild_from_gateways(&[create_test_gateway(
        "gw",
        "default",
        vec![create_test_listener("https", Some("rotate.example.com"), Some(strict))],
    )]);
    assert_eq!(
        matcher
            .match_sni(&test_listener_key(443), "rotate.example.com")
            .unwrap()
            .frontend_mtls,
        FrontendMtlsOutcome::StrictEffective
    );

    let insecure = with_frontend(
        make_tls_config("server", None),
        Some(FrontendValidationMode::AllowInsecureFallback),
        vec![frontend_ref("missing")],
        None,
    );
    matcher.rebuild_from_gateways(&[create_test_gateway(
        "gw",
        "default",
        vec![create_test_listener(
            "https",
            Some("rotate.example.com"),
            Some(insecure),
        )],
    )]);
    let entry = matcher
        .match_sni(&test_listener_key(443), "rotate.example.com")
        .unwrap();
    assert_eq!(entry.frontend_mtls, FrontendMtlsOutcome::InsecureFallback);
    assert!(entry.compiled_frontend_ca_store.is_none());
}

#[test]
fn frontend_eligibility_without_projected_validation_rejects() {
    let mut tls = make_tls_config("server", None);
    tls.frontend_matcher_eligibility = Some(FrontendMatcherEligibility::RequireExactEdgionTlsCertificate);
    let matcher = GatewayTlsMatcher::new();
    matcher.rebuild_from_gateways(&[create_test_gateway(
        "gw",
        "default",
        vec![create_test_listener(
            "https",
            Some("inconsistent.example.com"),
            Some(tls),
        )],
    )]);
    assert_eq!(
        matcher
            .match_sni(&test_listener_key(443), "inconsistent.example.com")
            .unwrap()
            .frontend_mtls,
        FrontendMtlsOutcome::StrictReject {
            reason: "frontend projection inconsistent"
        }
    );
}

#[test]
fn insecure_mode_cannot_bypass_authoritative_strict_reject() {
    let mut tls = with_frontend(
        make_tls_config("server", None),
        Some(FrontendValidationMode::AllowInsecureFallback),
        vec![frontend_ref("ca")],
        Some(vec![valid_ca_secret()]),
    );
    tls.frontend_matcher_eligibility = Some(FrontendMatcherEligibility::StrictReject);
    let matcher = GatewayTlsMatcher::new();
    matcher.rebuild_from_gateways(&[create_test_gateway(
        "gw",
        "default",
        vec![create_test_listener("https", Some("reject.example.com"), Some(tls))],
    )]);
    assert_eq!(
        matcher
            .match_sni(&test_listener_key(443), "reject.example.com")
            .unwrap()
            .frontend_mtls,
        FrontendMtlsOutcome::StrictReject {
            reason: "controller rejected gateway tls transport"
        }
    );
}

#[test]
fn serving_certificate_outcome_requires_exact_ordered_source_identity() {
    let references = vec![SecretObjectReference {
        group: None,
        kind: None,
        namespace: None,
        name: "expected".to_string(),
    }];
    let wrong_source = vec![ResolvedServingCertificateReference {
        index: 0,
        source: ServingCertificateSourceIdentity {
            group: String::new(),
            kind: "Secret".to_string(),
            namespace: "default".to_string(),
            name: "same-sized".to_string(),
        },
        secret: Some(valid_server_secret()),
        error: None,
    }];
    assert!(compile_serving_certificates(&references, Some(&wrong_source), "default").is_none());

    let mut wrong_payload = valid_server_secret();
    wrong_payload.metadata.name = Some("different-secret".to_string());
    let forged_identity = vec![ResolvedServingCertificateReference {
        index: 0,
        source: ServingCertificateSourceIdentity {
            group: String::new(),
            kind: "Secret".to_string(),
            namespace: "default".to_string(),
            name: "expected".to_string(),
        },
        secret: Some(wrong_payload),
        error: None,
    }];
    assert!(compile_serving_certificates(&references, Some(&forged_identity), "default").is_none());

    let exact = vec![ResolvedServingCertificateReference {
        index: 0,
        source: ServingCertificateSourceIdentity {
            group: String::new(),
            kind: "Secret".to_string(),
            namespace: "default".to_string(),
            name: "expected".to_string(),
        },
        secret: Some(valid_server_secret()),
        error: None,
    }];
    assert!(
        compile_serving_certificates(&references, Some(&exact), "default").is_some(),
        "a real matching certificate/private-key pair must compile"
    );

    let mut missing_namespace = valid_server_secret();
    missing_namespace.metadata.namespace = None;
    let missing_namespace = vec![ResolvedServingCertificateReference {
        secret: Some(missing_namespace),
        ..exact[0].clone()
    }];
    assert!(compile_serving_certificates(&references, Some(&missing_namespace), "default").is_none());
}

#[test]
fn strict_reject_and_serve_candidates_share_collision_precedence() {
    let reject = create_test_gateway(
        "alpha",
        "team-a",
        vec![create_test_listener(
            "https",
            Some("collision.example.com"),
            Some(with_frontend(
                make_tls_config("alpha-cert", None),
                None,
                vec![frontend_ref("missing")],
                None,
            )),
        )],
    );
    let serve = create_test_gateway(
        "beta",
        "team-b",
        vec![create_test_listener(
            "https",
            Some("collision.example.com"),
            Some(with_frontend(
                make_tls_config("beta-cert", None),
                None,
                vec![frontend_ref("valid")],
                Some(vec![valid_ca_secret()]),
            )),
        )],
    );
    for gateways in [vec![reject.clone(), serve.clone()], vec![serve.clone(), reject.clone()]] {
        let matcher = GatewayTlsMatcher::new();
        matcher.rebuild_from_gateways(&gateways);
        let winner = matcher
            .match_sni(&test_listener_key(443), "collision.example.com")
            .unwrap();
        assert_eq!(winner.attachment.gateway_name, "alpha");
        assert_eq!(
            winner.frontend_mtls,
            FrontendMtlsOutcome::StrictReject {
                reason: "ca unresolved"
            }
        );
    }
}

#[test]
fn test_rebuild_from_gateways() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let listener = create_test_listener(
        "https",
        Some("example.com"),
        Some(make_tls_config("test-cert", Some("default"))),
    );
    let proof = GatewayAttachmentProof {
        instance: "gateway-uid".to_string(),
        generation: 7,
        epoch: "resource-version-9".to_string(),
        attachment_digest: "a".repeat(64),
    };
    let mut gateway = create_test_gateway("test-gw", "default", vec![listener]);
    gateway.spec.resolved_attachment_proof = Some(proof.clone());
    let expected_attachment = ResolvedListenerAttachment {
        gateway_namespace: "default".to_string(),
        gateway_name: "test-gw".to_string(),
        listener_name: "https".to_string(),
        listener_key: key.clone(),
        gateway_proof: proof,
    };

    matcher.rebuild_from_gateways(&[gateway]);

    // Test exact match
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.attachment, expected_attachment);
    assert_eq!(entry.certificate_refs.len(), 1);
}

#[test]
fn test_no_tls_config() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let listener = create_test_listener("http", Some("example.com"), None);
    let gateway = create_test_gateway("test-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Should not find any entry
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none());
}

#[test]
fn test_no_hostname() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "test-cert".to_string(),
            namespace: None,
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };

    let listener = create_test_listener("https", None, Some(tls_config));
    let gateway = create_test_gateway("test-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Hostname-less listeners serve as catch-all fallback for any SNI on the same listener.
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_some(), "Catch-all listener should match any SNI");
    let entry = result.unwrap();
    assert_eq!(entry.attachment.gateway_name, "test-gw");
    assert_eq!(entry.attachment.listener_name, "https");

    // Should also match other arbitrary SNIs
    let result = matcher.match_sni(&key, "anything.foo.bar");
    assert!(result.is_some(), "Catch-all listener should match any SNI");
}

#[test]
fn test_wildcard_match() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "wildcard-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };

    // Gateway with wildcard hostname
    let listener = create_test_listener("https", Some("*.example.com"), Some(tls_config));
    let gateway = create_test_gateway("wildcard-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Test wildcard match - subdomain should match
    let result = matcher.match_sni(&key, "api.example.com");
    assert!(result.is_some(), "api.example.com should match *.example.com");
    let entry = result.unwrap();
    assert_eq!(entry.attachment.gateway_name, "wildcard-gw");
    assert_eq!(entry.certificate_refs[0].name, "wildcard-cert");

    // Another subdomain should also match
    let result = matcher.match_sni(&key, "www.example.com");
    assert!(result.is_some(), "www.example.com should match *.example.com");

    // Root domain should NOT match wildcard
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none(), "example.com should NOT match *.example.com");

    // RFC 6125 §6.4.3: deeper subdomain MUST NOT match a single-label wildcard
    let result = matcher.match_sni(&key, "a.b.example.com");
    assert!(
        result.is_none(),
        "a.b.example.com should NOT match *.example.com (RFC 6125 §6.4.3)"
    );
}

#[test]
fn test_multiple_gateways_multiple_listeners() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // Gateway 1: api.example.com
    let tls_config1 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "api-cert".to_string(),
            namespace: Some("prod".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };
    let listener1 = create_test_listener("api-https", Some("api.example.com"), Some(tls_config1));
    let gateway1 = create_test_gateway("api-gateway", "prod", vec![listener1]);

    // Gateway 2: Multiple listeners (www and admin)
    let tls_config2 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "www-cert".to_string(),
            namespace: Some("prod".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };
    let tls_config3 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "admin-cert".to_string(),
            namespace: Some("prod".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };
    let listener2 = create_test_listener("www-https", Some("www.example.com"), Some(tls_config2));
    let listener3 = create_test_listener("admin-https", Some("admin.example.com"), Some(tls_config3));
    let gateway2 = create_test_gateway("web-gateway", "prod", vec![listener2, listener3]);

    matcher.rebuild_from_gateways(&[gateway1, gateway2]);

    // Test api.example.com
    let result = matcher.match_sni(&key, "api.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.attachment.gateway_name, "api-gateway");
    assert_eq!(entry.certificate_refs[0].name, "api-cert");

    // Test www.example.com
    let result = matcher.match_sni(&key, "www.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.attachment.gateway_name, "web-gateway");
    assert_eq!(entry.attachment.listener_name, "www-https");
    assert_eq!(entry.certificate_refs[0].name, "www-cert");

    // Test admin.example.com
    let result = matcher.match_sni(&key, "admin.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();
    assert_eq!(entry.attachment.gateway_name, "web-gateway");
    assert_eq!(entry.attachment.listener_name, "admin-https");
    assert_eq!(entry.certificate_refs[0].name, "admin-cert");

    // Test unknown domain
    let result = matcher.match_sni(&key, "unknown.example.com");
    assert!(result.is_none());
}

#[test]
fn test_certificate_ref_namespace_fallback() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // Certificate ref without explicit namespace should use Gateway's namespace
    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "cert-without-ns".to_string(),
            namespace: None, // No namespace specified
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };

    let listener = create_test_listener("https", Some("test.example.com"), Some(tls_config));
    let gateway = create_test_gateway("test-gw", "my-namespace", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    let result = matcher.match_sni(&key, "test.example.com");
    assert!(result.is_some());
    let entry = result.unwrap();

    // Verify Gateway namespace is captured for later Secret lookup
    assert_eq!(entry.attachment.gateway_namespace, "my-namespace");
    // Certificate ref has no explicit namespace
    assert!(entry.certificate_refs[0].namespace.is_none());
}

#[test]
fn test_empty_matcher_fast_path() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // New matcher should be empty
    assert!(matcher.is_empty(), "New matcher should be empty");

    // Match should fail immediately without HashMap lookup
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none());

    // Rebuild with empty gateway list
    matcher.rebuild_from_gateways(&[]);
    assert!(matcher.is_empty(), "Matcher should still be empty after empty rebuild");

    // Rebuild with gateway that has no TLS
    let listener = create_test_listener("http", Some("example.com"), None);
    let gateway = create_test_gateway("no-tls-gw", "default", vec![listener]);
    matcher.rebuild_from_gateways(&[gateway]);
    assert!(matcher.is_empty(), "Matcher should be empty when gateway has no TLS");

    // Match should still fail fast
    let result = matcher.match_sni(&key, "example.com");
    assert!(result.is_none());
}

#[test]
fn test_has_entries_flag_transitions() {
    let matcher = GatewayTlsMatcher::new();

    // Initially empty
    assert!(matcher.is_empty());

    // Add TLS config
    let tls_config = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "test-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };
    let listener = create_test_listener("https", Some("example.com"), Some(tls_config));
    let gateway = create_test_gateway("test-gw", "default", vec![listener]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Now has entries
    assert!(
        !matcher.is_empty(),
        "Matcher should have entries after adding TLS config"
    );

    // Remove all gateways
    matcher.rebuild_from_gateways(&[]);

    // Back to empty
    assert!(
        matcher.is_empty(),
        "Matcher should be empty after removing all gateways"
    );
}

#[test]
fn test_listener_dimension_matching() {
    let matcher = GatewayTlsMatcher::new();
    let key_443 = test_listener_key(443);
    let key_8443 = test_listener_key(8443);
    let key_9443 = test_listener_key(9443);

    // Same hostname on different ports with different certs
    let tls_config_443 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "cert-443".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };

    let tls_config_8443 = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "cert-8443".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };

    let listener_443 = create_test_listener_with_port("https-443", 443, Some("api.example.com"), Some(tls_config_443));
    let listener_8443 =
        create_test_listener_with_port("https-8443", 8443, Some("api.example.com"), Some(tls_config_8443));

    let gateway = create_test_gateway("multi-port-gw", "default", vec![listener_443, listener_8443]);

    matcher.rebuild_from_gateways(&[gateway]);

    // Test listener on port 443
    let result = matcher.match_sni(&key_443, "api.example.com");
    assert!(result.is_some(), "Should find cert for listener on port 443");
    let entry = result.unwrap();
    assert_eq!(entry.attachment.listener_key, key_443);
    assert_eq!(entry.certificate_refs[0].name, "cert-443");

    // Test listener on port 8443
    let result = matcher.match_sni(&key_8443, "api.example.com");
    assert!(result.is_some(), "Should find cert for listener on port 8443");
    let entry = result.unwrap();
    assert_eq!(entry.attachment.listener_key, key_8443);
    assert_eq!(entry.certificate_refs[0].name, "cert-8443");

    // Test non-existent listener
    let result = matcher.match_sni(&key_9443, "api.example.com");
    assert!(result.is_none(), "Should not find cert for listener on port 9443");
}

#[test]
fn test_multiple_listeners_different_hostnames() {
    let matcher = GatewayTlsMatcher::new();
    let key_443 = test_listener_key(443);
    let key_8443 = test_listener_key(8443);

    let tls_config_api = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "api-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };

    let tls_config_admin = GatewayTLSConfig {
        mode: Some(GatewayTlsMode::Terminate),
        certificate_refs: Some(vec![SecretObjectReference {
            name: "admin-cert".to_string(),
            namespace: Some("default".to_string()),
            group: None,
            kind: None,
        }]),
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: Some(FrontendMatcherEligibility::Programmed),
    };

    // Port 443: api.example.com
    let listener_api = create_test_listener_with_port("api-https", 443, Some("api.example.com"), Some(tls_config_api));
    // Port 8443: admin.example.com
    let listener_admin =
        create_test_listener_with_port("admin-https", 8443, Some("admin.example.com"), Some(tls_config_admin));

    let gateway = create_test_gateway("mixed-gw", "default", vec![listener_api, listener_admin]);

    matcher.rebuild_from_gateways(&[gateway]);

    // api.example.com on listener 443
    let result = matcher.match_sni(&key_443, "api.example.com");
    assert!(result.is_some());
    assert_eq!(result.unwrap().certificate_refs[0].name, "api-cert");

    // admin.example.com on listener 8443
    let result = matcher.match_sni(&key_8443, "admin.example.com");
    assert!(result.is_some());
    assert_eq!(result.unwrap().certificate_refs[0].name, "admin-cert");

    // api.example.com on listener 8443 should fail (wrong listener)
    let result = matcher.match_sni(&key_8443, "api.example.com");
    assert!(result.is_none());

    // admin.example.com on listener 443 should fail (wrong listener)
    let result = matcher.match_sni(&key_443, "admin.example.com");
    assert!(result.is_none());
}

#[test]
fn test_catchall_with_hostname_priority() {
    let matcher = GatewayTlsMatcher::new();
    let key_443 = test_listener_key(443);
    let key_8443 = test_listener_key(8443);

    // Listener 1: specific hostname on port 443
    let tls_specific = make_tls_config("specific-cert", Some("default"));
    let listener_specific =
        create_test_listener_with_port("specific-https", 443, Some("api.example.com"), Some(tls_specific));

    // Listener 2: catch-all (no hostname) on same port 443
    let tls_catchall = make_tls_config("catchall-cert", Some("default"));
    let listener_catchall = create_test_listener_with_port("catchall-https", 443, None, Some(tls_catchall));

    let gateway = create_test_gateway("mixed-gw", "default", vec![listener_specific, listener_catchall]);
    matcher.rebuild_from_gateways(&[gateway]);

    // Exact hostname should use the specific cert, not catch-all
    let result = matcher.match_sni(&key_443, "api.example.com");
    assert!(result.is_some());
    assert_eq!(result.unwrap().certificate_refs[0].name, "specific-cert");

    // Unknown hostname should fall back to catch-all
    let result = matcher.match_sni(&key_443, "unknown.example.com");
    assert!(result.is_some(), "Unknown SNI should match catch-all");
    assert_eq!(result.unwrap().certificate_refs[0].name, "catchall-cert");

    // Different listener should not match either
    let result = matcher.match_sni(&key_8443, "unknown.example.com");
    assert!(result.is_none(), "Different listener should not match");
}

#[test]
fn test_programmed_false_without_explicit_transport_outcome_is_omitted() {
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::gateway::{GatewayStatus, ListenerStatus};

    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let mut tls_config = make_tls_config("server-cert", Some("default"));
    tls_config.frontend_matcher_eligibility = None;
    let listener = create_test_listener("https", Some("api.example.com"), Some(tls_config));

    let mut gateway = create_test_gateway("test-gw", "default", vec![listener]);
    gateway.spec.current_status = Some(GatewayStatus {
        addresses: None,
        conditions: None,
        listeners: Some(vec![ListenerStatus {
            name: "https".to_string(),
            supported_kinds: vec![],
            attached_routes: 0,
            conditions: vec![Condition {
                last_transition_time: "2024-01-01T00:00:00Z".to_string(),
                message: String::new(),
                observed_generation: None,
                reason: "Invalid".to_string(),
                status: "False".to_string(),
                type_: "Programmed".to_string(),
            }],
        }]),
    });

    matcher.rebuild_from_gateways(&[gateway]);

    // Gateway status is not an authoritative transport signal. Only the
    // Controller-private typed outcome may retain a StrictReject slot.
    assert!(matcher.match_sni(&key, "api.example.com").is_none());
}

#[test]
fn test_exact_hostname_not_merged_into_wildcard_bucket() {
    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    let tls_wildcard = make_tls_config("wildcard-cert", Some("default"));
    let tls_exact = make_tls_config("exact-cert", Some("default"));

    let listener_wc = create_test_listener("https-wc", Some("*.wildcard.org"), Some(tls_wildcard));
    let listener_exact = create_test_listener("https-exact", Some("fourth-example.wildcard.org"), Some(tls_exact));

    let gateway = create_test_gateway("test-gw", "default", vec![listener_wc, listener_exact]);
    matcher.rebuild_from_gateways(&[gateway]);

    // Exact hostname must match the exact listener, not the wildcard one
    let result = matcher.match_sni(&key, "fourth-example.wildcard.org");
    assert!(result.is_some(), "exact hostname should match");
    let entry = result.unwrap();
    assert_eq!(
        entry.attachment.listener_name, "https-exact",
        "exact hostname must select the exact listener, not the wildcard"
    );
    assert_eq!(entry.certificate_refs[0].name, "exact-cert");

    // Other subdomains should still match the wildcard
    let result = matcher.match_sni(&key, "other.wildcard.org");
    assert!(result.is_some());
    assert_eq!(result.unwrap().attachment.listener_name, "https-wc");
}

#[test]
fn test_cross_gateway_sni_collision_is_deterministic() {
    // Two Gateways register TLS for the SAME hostname on the SAME listener.
    // The controller's conflict detection is intra-gateway only, so this is not
    // admission-blocked. match_sni must pick the same winner regardless of the
    // order the Gateways arrive in the rebuild slice (no HashMap-order dependence).
    let key = test_listener_key(443);

    let gw_a = create_test_gateway(
        "alpha-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("alpha-cert", Some("team-a"))),
        )],
    );
    let gw_b = create_test_gateway(
        "beta-gw",
        "team-b",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("beta-cert", Some("team-b"))),
        )],
    );

    // Order 1: A then B
    let matcher1 = GatewayTlsMatcher::new();
    matcher1.rebuild_from_gateways(&[gw_a.clone(), gw_b.clone()]);
    let winner1 = matcher1.match_sni(&key, "shared.example.com").unwrap();

    // Order 2: B then A
    let matcher2 = GatewayTlsMatcher::new();
    matcher2.rebuild_from_gateways(&[gw_b, gw_a]);
    let winner2 = matcher2.match_sni(&key, "shared.example.com").unwrap();

    // Same deterministic winner irrespective of input order
    assert_eq!(
        winner1.attachment.gateway_namespace,
        winner2.attachment.gateway_namespace
    );
    assert_eq!(winner1.attachment.gateway_name, winner2.attachment.gateway_name);
    // Neither Gateway sets a creationTimestamp (None), so the tiebreak falls back to
    // (namespace, name): "team-a" < "team-b".
    assert_eq!(winner1.attachment.gateway_namespace, "team-a");
    assert_eq!(winner1.attachment.gateway_name, "alpha-gw");
    assert_eq!(winner1.certificate_refs[0].name, "alpha-cert");
}

#[test]
fn test_cross_gateway_collision_prefers_oldest_creation_timestamp() {
    // The tiebreak is creationTimestamp-first (oldest wins), then namespace/name.
    // Give the lexically-LARGER-named Gateway the OLDER timestamp; it must still win,
    // proving creationTimestamp dominates the name comparison.
    use chrono::{TimeZone, Utc};
    let key = test_listener_key(443);

    let older = Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
    let newer = Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());

    let mut incumbent = create_test_gateway(
        "zzz-gw",
        "team-z",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("incumbent-cert", Some("team-z"))),
        )],
    );
    incumbent.metadata.creation_timestamp = Some(older);

    let mut newcomer = create_test_gateway(
        "aaa-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            Some("shared.example.com"),
            Some(make_tls_config("newcomer-cert", Some("team-a"))),
        )],
    );
    newcomer.metadata.creation_timestamp = Some(newer);

    for order in [
        vec![incumbent.clone(), newcomer.clone()],
        vec![newcomer.clone(), incumbent.clone()],
    ] {
        let matcher = GatewayTlsMatcher::new();
        matcher.rebuild_from_gateways(&order);
        let winner = matcher.match_sni(&key, "shared.example.com").unwrap();
        assert_eq!(
            winner.attachment.gateway_name, "zzz-gw",
            "older Gateway must win regardless of name or input order"
        );
        assert_eq!(winner.certificate_refs[0].name, "incumbent-cert");
    }
}

#[test]
fn test_cross_gateway_wildcard_collision_is_deterministic() {
    // Same scenario for an identical *wildcard* hostname on one listener. Before
    // the get_mut_exact wildcard fix, the second Gateway silently overwrote the
    // first (last-in-slice wins) — non-deterministic by input order.
    let key = test_listener_key(443);

    let gw_a = create_test_gateway(
        "alpha-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            Some("*.example.com"),
            Some(make_tls_config("alpha-cert", Some("team-a"))),
        )],
    );
    let gw_b = create_test_gateway(
        "beta-gw",
        "team-b",
        vec![create_test_listener(
            "https",
            Some("*.example.com"),
            Some(make_tls_config("beta-cert", Some("team-b"))),
        )],
    );

    let matcher1 = GatewayTlsMatcher::new();
    matcher1.rebuild_from_gateways(&[gw_a.clone(), gw_b.clone()]);
    let winner1 = matcher1.match_sni(&key, "foo.example.com").unwrap();

    let matcher2 = GatewayTlsMatcher::new();
    matcher2.rebuild_from_gateways(&[gw_b, gw_a]);
    let winner2 = matcher2.match_sni(&key, "foo.example.com").unwrap();

    assert_eq!(winner1.attachment.gateway_name, winner2.attachment.gateway_name);
    assert_eq!(winner1.attachment.gateway_name, "alpha-gw");
    assert_eq!(winner1.certificate_refs[0].name, "alpha-cert");
}

#[test]
fn test_cross_gateway_catchall_collision_is_deterministic() {
    // Same scenario for hostname-less catch-all listeners colliding on one listener.
    let key = test_listener_key(443);

    let gw_a = create_test_gateway(
        "alpha-gw",
        "team-a",
        vec![create_test_listener(
            "https",
            None,
            Some(make_tls_config("alpha-cert", Some("team-a"))),
        )],
    );
    let gw_b = create_test_gateway(
        "beta-gw",
        "team-b",
        vec![create_test_listener(
            "https",
            None,
            Some(make_tls_config("beta-cert", Some("team-b"))),
        )],
    );

    let matcher1 = GatewayTlsMatcher::new();
    matcher1.rebuild_from_gateways(&[gw_a.clone(), gw_b.clone()]);
    let winner1 = matcher1.match_sni(&key, "anything.example.com").unwrap();

    let matcher2 = GatewayTlsMatcher::new();
    matcher2.rebuild_from_gateways(&[gw_b, gw_a]);
    let winner2 = matcher2.match_sni(&key, "anything.example.com").unwrap();

    assert_eq!(winner1.attachment.gateway_name, winner2.attachment.gateway_name);
    assert_eq!(winner1.attachment.gateway_name, "alpha-gw");
    assert_eq!(winner1.certificate_refs[0].name, "alpha-cert");
}

#[test]
fn test_programmed_false_does_not_shadow_valid_listener() {
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::gateway::{GatewayStatus, ListenerStatus};

    let matcher = GatewayTlsMatcher::new();
    let key = test_listener_key(443);

    // Invalid gateway: Programmed=False, catch-all (no hostname), valid cert
    let mut invalid_tls = make_tls_config("invalid-cert", Some("default"));
    invalid_tls.frontend_matcher_eligibility = Some(FrontendMatcherEligibility::StrictReject);
    let invalid_listener = create_test_listener("https", None, Some(invalid_tls));
    let mut invalid_gw = create_test_gateway("invalid-gw", "default", vec![invalid_listener]);
    invalid_gw.spec.current_status = Some(GatewayStatus {
        addresses: None,
        conditions: None,
        listeners: Some(vec![ListenerStatus {
            name: "https".to_string(),
            supported_kinds: vec![],
            attached_routes: 0,
            conditions: vec![Condition {
                last_transition_time: "2024-01-01T00:00:00Z".to_string(),
                message: String::new(),
                observed_generation: None,
                reason: "NoValidCACertificate".to_string(),
                status: "False".to_string(),
                type_: "Programmed".to_string(),
            }],
        }]),
    });

    // Valid gateway: Programmed=True, same port, specific hostname
    let valid_tls = make_tls_config("valid-cert", Some("default"));
    let valid_listener = create_test_listener("https", Some("api.example.com"), Some(valid_tls));
    let valid_gw = create_test_gateway("valid-gw", "default", vec![valid_listener]);

    matcher.rebuild_from_gateways(&[invalid_gw, valid_gw]);

    // Valid listener should still match
    let result = matcher.match_sni(&key, "api.example.com");
    assert!(result.is_some(), "valid listener should still be registered");
    assert_eq!(result.unwrap().attachment.gateway_name, "valid-gw");

    // The invalid catch-all remains an explicit rejection entry, never a
    // serving fallback and never an opportunity for EdgionTls rescue.
    let fallback = matcher
        .match_sni(&key, "unknown.example.com")
        .expect("invalid catch-all should remain fail-closed");
    assert!(matches!(
        fallback.frontend_mtls,
        FrontendMtlsOutcome::StrictReject {
            reason: "controller rejected gateway tls transport"
        }
    ));
}

use super::*;
use k8s_openapi::api::core::v1::Secret;
use std::net::IpAddr;
use std::sync::Arc;

#[test]
fn backend_websocket_label_is_allocation_free_and_preserves_existing_classification() {
    for existing in [None, Some("grpc"), Some("grpc-web"), Some("websocket")] {
        let mut peer = Box::new(HttpPeer::new("127.0.0.1:8080", false, String::new()));
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.discover_protocol = existing;
        let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
            apply_backend_app_protocol(&mut peer, Some(BackendAppProtocol::WebSocket), &mut ctx);
        });
        assert_eq!(allocations, 0);
        assert_eq!(ctx.request_info.discover_protocol, existing.or(Some("websocket")));
        assert!(!ctx.request_info.is_grpc_request);
    }
}

#[test]
fn tls_projection_borrows_policy_and_log_retains_its_own_sni() {
    let policy: BackendTLSPolicy = serde_json::from_value(serde_json::json!({
        "metadata":{"namespace":"default","name":"tls"},
        "spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend"}],
        "validation":{"hostname":"backend.example","wellKnownCACertificates":"System"},
        "useSystemCa":true}
    }))
    .unwrap();
    let lookup = BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy));
    let (use_tls, sni) = extract_tls_config(&lookup);
    assert!(use_tls);
    assert_eq!(sni, "backend.example");
    assert_eq!(
        sni.as_ptr(),
        lookup.effective().unwrap().policy.spec.validation.hostname.as_ptr()
    );

    let mut ctx = EdgionHttpContext::new();
    ctx.init_backend_context(
        crate::Rsc::new("Service", "default", "backend"),
        "default/backend".to_string(),
        crate::backends::ServiceBackendKind::Service,
        Some(0),
    );
    ctx.push_upstream(None, None);
    record_tls_to_upstream(&mut ctx, use_tls, sni);
    drop(lookup);
    assert_eq!(
        ctx.get_current_upstream().unwrap().tls.as_ref().unwrap().sni.as_deref(),
        Some("backend.example")
    );

    for lookup in [BackendTlsPolicyLookup::Absent, BackendTlsPolicyLookup::Invalid] {
        record_tls_to_upstream(&mut ctx, true, "previous.example");
        let (use_tls, sni) = extract_tls_config(&lookup);
        assert_eq!((use_tls, sni), (false, ""));
        record_tls_to_upstream(&mut ctx, use_tls, sni);
        assert!(ctx.get_current_upstream().unwrap().tls.is_none());
    }
}

#[cfg(unix)]
#[test]
fn endpoint_boundary_rejects_unix_even_with_loopback_override() {
    let addr =
        SocketAddr::Unix(std::os::unix::net::SocketAddr::from_pathname("/tmp/edgion-endpoint-test.sock").unwrap());
    assert!(classify_backend_addr(&addr).is_none());
    assert_eq!(
        validate_endpoint_backend_addr(&addr, false),
        Err(EdgionErrorCode::BackendPortResolutionFailed)
    );
    assert_eq!(
        validate_endpoint_backend_addr(&addr, true),
        Err(EdgionErrorCode::BackendPortResolutionFailed)
    );
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn first_safe_inet_addr_skips_unsafe_and_picks_public() {
    // Loopback + link-local are unsafe; the public IP is selected.
    let ips = [ip("127.0.0.1"), ip("169.254.169.254"), ip("93.184.216.34")];
    let addr = first_safe_inet_addr(&ips, 443, false).unwrap();
    match addr {
        SocketAddr::Inet(a) => {
            assert_eq!(a.ip(), ip("93.184.216.34"));
            assert_eq!(a.port(), 443);
        }
        _ => panic!("expected inet addr"),
    }
}

#[test]
fn first_safe_inet_addr_all_unsafe_errors() {
    let ips = [ip("127.0.0.1"), ip("169.254.169.254")];
    let err = first_safe_inet_addr(&ips, 80, false).unwrap_err();
    // The last candidate's rejection status surfaces (link-local here).
    assert_eq!(err, EdgionErrorCode::BackendAddrLinkLocalNotAllowed);
}

#[test]
fn first_safe_inet_addr_loopback_allowed_when_flag_set() {
    let ips = [ip("127.0.0.1")];
    let addr = first_safe_inet_addr(&ips, 8080, true).unwrap();
    assert!(matches!(addr, SocketAddr::Inet(_)));
}

#[test]
fn first_safe_inet_addr_empty_is_dns_failure() {
    let err = first_safe_inet_addr(&[], 80, false).unwrap_err();
    assert_eq!(err, EdgionErrorCode::BackendDnsResolutionFailed);
}

#[test]
fn client_identity_requires_policy_and_prefers_policy_identity() {
    use ClientIdentitySource::{Gateway, Policy};

    let cases = [
        (false, false, false, None),
        (false, false, true, None),
        (false, true, false, None),
        (false, true, true, None),
        (true, false, false, None),
        (true, false, true, Some(Gateway)),
        (true, true, false, Some(Policy)),
        (true, true, true, Some(Policy)),
    ];

    for (policy_present, policy_identity_present, gateway_identity_present, expected) in cases {
        assert_eq!(
            select_client_identity_source(policy_present, policy_identity_present, gateway_identity_present),
            expected
        );
    }
}

#[test]
fn backend_app_protocol_aliases_are_ascii_case_insensitive() {
    let cases = [
        ("KuBeRnEtEs.Io/H2C", Some(BackendAppProtocol::H2c)),
        ("H2C", Some(BackendAppProtocol::H2c)),
        ("KuBeRnEtEs.Io/Ws", Some(BackendAppProtocol::WebSocket)),
        ("Ws", Some(BackendAppProtocol::WebSocket)),
        ("KuBeRnEtEs.Io/WsS", Some(BackendAppProtocol::WebSocket)),
        ("WsS", Some(BackendAppProtocol::WebSocket)),
        ("KuBeRnEtEs.Io/Unknown", None),
    ];

    for (value, expected) in cases {
        assert_eq!(parse_backend_app_protocol(value), expected, "value={value}");
    }
}

#[cfg(feature = "boringssl")]
fn client_identity_secret(cert: Option<&str>, key: Option<&str>) -> Secret {
    let mut data = std::collections::BTreeMap::new();
    if let Some(cert) = cert {
        data.insert(CERT.to_string(), k8s_openapi::ByteString(cert.as_bytes().to_vec()));
    }
    if let Some(key) = key {
        data.insert(KEY.to_string(), k8s_openapi::ByteString(key.as_bytes().to_vec()));
    }
    Secret {
        data: Some(data),
        ..Default::default()
    }
}

#[cfg(feature = "boringssl")]
fn generated_cert_and_key(name: &str) -> (String, String) {
    let key = rcgen::KeyPair::generate().expect("test key generation must succeed");
    let params =
        rcgen::CertificateParams::new(vec![name.to_string()]).expect("test certificate parameters must be valid");
    let cert = params.self_signed(&key).expect("test certificate signing must succeed");
    (cert.pem(), key.serialize_pem())
}

#[cfg(feature = "boringssl")]
#[test]
fn client_identity_preserves_single_and_multi_certificate_chains() {
    let (leaf_pem, leaf_key) = generated_cert_and_key("leaf.example");
    let (intermediate_pem, _) = generated_cert_and_key("intermediate.example");

    let single = build_client_cert_key_from_secret(&client_identity_secret(Some(&leaf_pem), Some(&leaf_key)))
        .expect("single certificate identity must parse");
    assert!(single.intermediates().is_empty());

    let bundle = format!("{leaf_pem}{intermediate_pem}");
    let chain = build_client_cert_key_from_secret(&client_identity_secret(Some(&bundle), Some(&leaf_key)))
        .expect("certificate chain identity must parse");
    assert_eq!(chain.intermediates().len(), 1);

    let expected_leaf = X509::from_pem(leaf_pem.as_bytes()).expect("leaf certificate must parse");
    let expected_intermediate =
        X509::from_pem(intermediate_pem.as_bytes()).expect("intermediate certificate must parse");
    assert_eq!(
        chain.leaf().to_der().expect("leaf DER"),
        expected_leaf.to_der().expect("expected leaf DER")
    );
    assert_eq!(
        chain.intermediates()[0].to_der().expect("intermediate DER"),
        expected_intermediate.to_der().expect("expected intermediate DER")
    );
}

#[cfg(feature = "boringssl")]
#[test]
fn client_identity_rejects_missing_empty_and_malformed_material() {
    let (cert_pem, key_pem) = generated_cert_and_key("client.example");
    let cases = [
        Secret::default(),
        client_identity_secret(None, Some(&key_pem)),
        client_identity_secret(Some(&cert_pem), None),
        client_identity_secret(Some(""), Some(&key_pem)),
        client_identity_secret(Some("not a certificate"), Some(&key_pem)),
        client_identity_secret(Some(&cert_pem), Some("")),
        client_identity_secret(Some(&cert_pem), Some("not a private key")),
    ];

    for secret in cases {
        assert!(matches!(
            build_client_cert_key_from_secret(&secret),
            Err(EdgionErrorCode::Unknown)
        ));
    }
}

#[test]
fn gateway_identity_does_not_enable_tls_without_policy() {
    crate::test_subprocess::assert_in_subprocess(
        "backends::peer_builder::tests",
        "gateway_identity_does_not_enable_tls_without_policy",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "gateway_identity_does_not_enable_tls_without_policy",
        || {
            let gateway_key = "test/gateway-identity-without-policy";
            let store = get_global_gateway_backend_tls_store();
            let gateway = gateway_identity_owner("test", "gateway-identity-without-policy", Secret::default());
            store.update_from_gateway(gateway_key, &gateway);
            assert!(
                store.get(gateway_key).is_some(),
                "the no-policy control must carry an actual stored identity"
            );

            let result = peer_from_backend_tls_lookup(
                SocketAddr::Inet("127.0.0.1:443".parse().unwrap()),
                &BackendTlsPolicyLookup::Absent,
                Some(gateway_key),
            );
            store.remove(gateway_key);

            let peer = result.expect("Gateway identity must not be parsed without a BackendTLSPolicy");
            assert!(!peer.is_tls());
            assert!(peer.client_cert_key.is_none());
        },
    );
}

#[test]
fn invalid_lookup_does_not_build_a_plaintext_peer() {
    let err = peer_from_backend_tls_lookup(
        SocketAddr::Inet("127.0.0.1:443".parse().unwrap()),
        &BackendTlsPolicyLookup::Invalid,
        None,
    )
    .unwrap_err();
    assert_eq!(err, EdgionErrorCode::BackendTlsPolicyInvalid);
}

#[test]
fn effective_peer_sets_profile_group_key_without_sans() {
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
    };
    use edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    let policy = Arc::new(BackendTLSPolicy {
        metadata: ObjectMeta {
            namespace: Some("tenant".to_string()),
            name: Some("tls".to_string()),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: None,
                },
            ),
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: String::new(),
                kind: "Service".to_string(),
                name: "backend".to_string(),
                section_name: None,
            }],
            resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: None,
                hostname: "backend.example.com".to_string(),
                subject_alt_names: None,
                well_known_ca_certificates: None,
            },
            options: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: true,
        },
        status: None,
    });
    let effective = EffectiveBackendTlsPolicy::from_policy(policy);
    let peer = create_tls_peer(SocketAddr::Inet("127.0.0.1:443".parse().unwrap()), &effective, None)
        .expect("system-CA policy must build a TLS peer");
    assert!(peer.is_tls());
    assert_eq!(peer.group_key, effective.profile.group_key());
}

pub(super) fn gateway_identity_owner(namespace: &str, name: &str, mut secret: Secret) -> edgion_resources::Gateway {
    secret.metadata.name = Some("client".into());
    secret.metadata.namespace = Some(namespace.into());
    serde_json::from_value(serde_json::json!({
        "metadata":{"name":name,"namespace":namespace},"spec":{"gatewayClassName":"public",
        "tls":{"backend":{"clientCertificateRef":{"name":"client"},"resolvedClientCertificate":secret}}}
    }))
    .unwrap()
}

#[test]
fn gateway_identity_pairing_gates_actual_peer_fallback_without_changing_policy_precedence() {
    crate::test_subprocess::assert_in_subprocess(
        "backends::peer_builder::tests",
        "gateway_identity_pairing_gates_actual_peer_fallback_without_changing_policy_precedence",
        "EDGION_R11_GATEWAY_PEER_CHILD",
        "publication",
        || {
            let key = "r11-peer/edge";
            let store = get_global_gateway_backend_tls_store();
            let (cert, private_key) = generated_cert_and_key("client.example");
            let mut identity = client_identity_secret(Some(&cert), Some(&private_key));
            identity.metadata.resource_version = Some("same".into());
            let owner = gateway_identity_owner("r11-peer", "edge", identity.clone());
            let mut policy: BackendTLSPolicy = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"policy","namespace":"r11-peer"},"spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend"}],
        "validation":{"hostname":"backend.example"},"useSystemCa":true}
    })).unwrap();
            let make_peer = |policy: BackendTLSPolicy| {
                create_tls_peer(
                    SocketAddr::Inet("127.0.0.1:443".parse().unwrap()),
                    &EffectiveBackendTlsPolicy::from_policy(Arc::new(policy)),
                    Some(key),
                )
                .unwrap()
            };
            store.update_from_gateway(key, &owner);
            assert!(make_peer(policy.clone()).client_cert_key.is_some());
            let mut mismatch = owner.clone();
            mismatch
                .spec
                .tls
                .as_mut()
                .unwrap()
                .backend
                .as_mut()
                .unwrap()
                .resolved_client_certificate
                .as_mut()
                .unwrap()
                .metadata
                .name = Some("other".into());
            store.update_from_gateway(key, &mismatch);
            let anonymous = make_peer(policy.clone());
            assert!(anonymous.is_tls());
            assert!(anonymous.client_cert_key.is_none());
            policy.spec.resolved_client_certificate = Some(identity);
            assert!(make_peer(policy.clone()).client_cert_key.is_some());
            store.update_from_gateway(key, &owner);
            assert!(make_peer(policy).client_cert_key.is_some());
            store.remove(key);
        },
    );
}

#[test]
fn policy_ancestor_refusal_blocks_actual_peer_construction() {
    let mut policy: BackendTLSPolicy = serde_json::from_value(serde_json::json!({
        "metadata":{"namespace":"capacity-peer","name":"tls"},"spec":{
        "targetRefs":[{"group":"","kind":"Service","name":"backend"}],
        "validation":{"hostname":"backend.example","wellKnownCACertificates":"System"},"useSystemCa":true}
    }))
    .unwrap();
    policy.spec.prepared_ancestor_admission = Some(
        edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Scoped(vec![
            "capacity-peer/represented".into(),
        ]),
    );
    let lookup = BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy));
    for key in [None, Some("capacity-peer/new")] {
        assert_eq!(
            peer_from_backend_tls_lookup(SocketAddr::Inet("127.0.0.1:443".parse().unwrap()), &lookup, key).unwrap_err(),
            EdgionErrorCode::BackendTlsPolicyInvalid
        );
    }
    assert!(peer_from_backend_tls_lookup(
        SocketAddr::Inet("127.0.0.1:443".parse().unwrap()),
        &lookup,
        Some("capacity-peer/represented")
    )
    .is_ok());
}

use super::*;
use edgion_resources::resources::backend_tls_policy::{
    BackendTLSPolicyCACertificateRef, BackendTLSPolicySpec, BackendTLSPolicyValidation,
};
use edgion_resources::resources::BackendTLSPolicy;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

#[test]
fn tls_hostname_cannot_bypass_the_endpoint_pin_as_a_numeric_host() {
    let address = "10.0.0.1:8443".parse().unwrap();
    for hostname in ["127.0.0.1", "2130706433", "0x7f000001", "0177.0.0.1", "[::1]"] {
        assert_eq!(
            probe_url(address, Some(hostname), "/health", None).unwrap_err(),
            "probe configuration unavailable"
        );
    }
    let url = probe_url(
        address,
        Some("backend.example.com"),
        "//other.example/path",
        Some("a=b#fragment"),
    )
    .unwrap();
    assert_eq!(url.host_str(), Some("backend.example.com"));
    assert_eq!(url.port(), Some(8443));
    assert_eq!(url.path(), "//other.example/path");
    assert_eq!(url.query(), Some("a=b%23fragment"));
    assert!(url.fragment().is_none());
}

#[test]
fn plaintext_ipv6_uses_the_proven_socket_and_preserves_escaped_paths() {
    let url = probe_url("[2001:db8::1]:8080".parse().unwrap(), None, "/foo%2Fbar", None).unwrap();
    assert_eq!(url.as_str(), "http://[2001:db8::1]:8080/foo%2Fbar");
}

#[test]
fn mixed_valid_and_empty_ca_maps_to_generic_probe_configuration_error() {
    let certificate = rcgen::generate_simple_self_signed(vec!["backend.example.com".to_string()])
        .expect("generate probe CA certificate");
    let ca_secret = |pem: &[u8]| Secret {
        data: Some(BTreeMap::from([("ca.crt".to_string(), ByteString(pem.to_vec()))])),
        ..Default::default()
    };
    let policy = BackendTLSPolicy {
        metadata: Default::default(),
        spec: BackendTLSPolicySpec {
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: None,
                },
            ),
            current_status: None,
            target_refs: Vec::new(),
            resolved_target_class: None,
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: Some(
                    ["ca-valid", "ca-empty"]
                        .map(|name| BackendTLSPolicyCACertificateRef {
                            group: String::new(),
                            kind: "Secret".to_string(),
                            name: name.to_string(),
                        })
                        .to_vec(),
                ),
                hostname: "backend.example.com".to_string(),
                subject_alt_names: None,
                well_known_ca_certificates: None,
            },
            options: None,
            resolved_ca_certificates: Some(vec![ca_secret(certificate.cert.pem().as_bytes()), ca_secret(b"")]),
            resolved_client_certificate: None,
            use_system_ca: false,
        },
        status: None,
    };
    let transport = ProbeTransport::new(
        BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy)),
        get_service_discovery_coordinator().load_generation(),
        get_global_backend_tls_policy_store().snapshot().generation(),
        None,
    );

    assert_eq!(transport.build_tls().unwrap_err(), "probe configuration unavailable");
}

#[test]
fn gateway_client_pairing_and_probe_capture_share_checked_publication() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::all_endpoint_status::transport::tests",
        "gateway_client_pairing_and_probe_capture_share_checked_publication",
        "EDGION_R11_GATEWAY_PROBE_CHILD",
        "publication",
        || {
            let key = "r11-probe/edge";
            let store = get_global_gateway_backend_tls_store();
            let cert = rcgen::generate_simple_self_signed(vec!["client.example".into()]).unwrap();
            let secret = Secret {
                metadata: kube::api::ObjectMeta {
                    name: Some("client".into()),
                    namespace: Some("r11-probe".into()),
                    resource_version: Some("same".into()),
                    ..Default::default()
                },
                data: Some(BTreeMap::from([
                    ("tls.crt".into(), ByteString(cert.cert.pem().into_bytes())),
                    (
                        "tls.key".into(),
                        ByteString(cert.signing_key.serialize_pem().into_bytes()),
                    ),
                ])),
                ..Default::default()
            };
            let owner: edgion_resources::Gateway = serde_json::from_value(serde_json::json!({
                "metadata":{"name":"edge","namespace":"r11-probe"},"spec":{"gatewayClassName":"public",
                "tls":{"backend":{"clientCertificateRef":{"name":"client"},"resolvedClientCertificate":secret}}}
            }))
            .unwrap();
            let policy: BackendTLSPolicy = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"policy","namespace":"r11-probe"},"spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend"}],
        "validation":{"hostname":"backend.example","wellKnownCACertificates":"System"},"useSystemCa":true}
    })).unwrap();
            let probe = |policy: BackendTLSPolicy| {
                ProbeTransport::new(
                    BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy)),
                    get_service_discovery_coordinator().load_generation(),
                    get_global_backend_tls_policy_store().snapshot().generation(),
                    Some(key),
                )
            };
            store.update_from_gateway(key, &owner);
            let captured = probe(policy.clone());
            assert!(captured.is_current());
            assert!(captured
                .build_tls()
                .unwrap()
                .unwrap()
                .client_auth_cert_resolver
                .has_certs());
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
            assert!(!captured.is_current());
            assert_eq!(
                captured.gateway_identity.as_ref().unwrap().metadata.name.as_deref(),
                Some("client")
            );
            assert!(captured
                .build_tls()
                .unwrap()
                .unwrap()
                .client_auth_cert_resolver
                .has_certs());
            let fresh = probe(policy.clone());
            assert!(fresh.is_current());
            assert!(!fresh
                .build_tls()
                .unwrap()
                .unwrap()
                .client_auth_cert_resolver
                .has_certs());
            let mut explicit = policy.clone();
            explicit.spec.resolved_client_certificate = Some(secret);
            let preferred = probe(explicit);
            assert!(preferred.gateway_key.is_none());
            assert!(preferred
                .build_tls()
                .unwrap()
                .unwrap()
                .client_auth_cert_resolver
                .has_certs());
            store.update_from_gateway(key, &owner);
            assert!(!fresh.is_current());
            assert!(preferred.is_current());
            assert!(probe(policy)
                .build_tls()
                .unwrap()
                .unwrap()
                .client_auth_cert_resolver
                .has_certs());
            store.remove(key);
        },
    );
}

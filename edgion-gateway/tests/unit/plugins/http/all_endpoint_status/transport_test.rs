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

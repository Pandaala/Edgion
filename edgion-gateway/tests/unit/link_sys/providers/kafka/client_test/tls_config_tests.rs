use super::build_kafka_tls_config;
use edgion_resources::constants::secret_keys::tls as tls_keys;
use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn install_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn gen_ca_leaf() -> (String, String, String) {
    let ca = rcgen::generate_simple_self_signed(vec!["kafka-ca".into()]).unwrap();
    let leaf = rcgen::generate_simple_self_signed(vec!["kafka.example.com".into()]).unwrap();
    (ca.cert.pem(), leaf.cert.pem(), leaf.signing_key.serialize_pem())
}

fn secret(data: &[(&str, &[u8])]) -> Secret {
    let m = data
        .iter()
        .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
        .collect::<BTreeMap<_, _>>();
    Secret {
        data: Some(m),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    }
}

fn ca_ref(name: &str) -> OutboundTlsLocal {
    OutboundTlsLocal {
        // Pin verify explicitly so the test does not depend on the shared
        // process-global outbound default that other tests may mutate.
        verify: Some(true),
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                group: String::new(),
                kind: "Secret".into(),
                name: name.into(),
                namespace: None,
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn tls_with(outbound: OutboundTlsLocal, ca: Option<Vec<Secret>>, client: Option<Secret>) -> TlsConfig {
    TlsConfig {
        enabled: true,
        outbound,
        resolved_ca_certificates: ca,
        resolved_client_certificate: client,
    }
}

/// enabled TLS with no custom CA and no client identity must still yield a
/// verifying system-roots config (rskafka has no implicit default TLS).
#[test]
fn system_roots_fallback_when_no_material() {
    install_provider();
    let tls = tls_with(
        OutboundTlsLocal {
            verify: Some(true),
            ..Default::default()
        },
        None,
        None,
    );
    assert!(
        build_kafka_tls_config(&tls).is_ok(),
        "enabled TLS with no custom material must build a system-roots config"
    );
}

#[test]
fn custom_ca_builds() {
    install_provider();
    let (ca, _c, _k) = gen_ca_leaf();
    let tls = tls_with(
        ca_ref("my-ca"),
        Some(vec![secret(&[(tls_keys::CA_CERT, ca.as_bytes())])]),
        None,
    );
    assert!(build_kafka_tls_config(&tls).is_ok());
}

#[test]
fn ca_and_client_mtls_builds() {
    install_provider();
    let (ca, cert, key) = gen_ca_leaf();
    let tls = tls_with(
        ca_ref("my-ca"),
        Some(vec![secret(&[(tls_keys::CA_CERT, ca.as_bytes())])]),
        Some(secret(&[("tls.crt", cert.as_bytes()), ("tls.key", key.as_bytes())])),
    );
    let cfg = build_kafka_tls_config(&tls).expect("CA + mTLS identity must build");
    assert!(
        cfg.client_auth_cert_resolver.has_certs(),
        "client cert must be installed"
    );
}

/// rskafka's rustls connector always verifies (provider_can_disable=false),
/// so verify=false must be rejected up front, never silently honored.
#[test]
fn verify_false_rejected() {
    install_provider();
    let tls = tls_with(
        OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        },
        None,
        None,
    );
    assert!(build_kafka_tls_config(&tls).is_err());
}

use super::*;
use edgion_resources::resources::backend_tls_policy::SubjectAltNameType;
use rustls::pki_types::ServerName;
use std::sync::Arc;

fn handshake(
    config: ClientConfig,
    certificate: &rcgen::CertifiedKey<rcgen::KeyPair>,
    hostname: &str,
) -> Result<(), rustls::Error> {
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.cert.der().clone()],
            PrivateKeyDer::from_pem_slice(certificate.signing_key.serialize_pem().as_bytes()).unwrap(),
        )
        .unwrap();
    let mut server = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
    let mut client =
        rustls::ClientConnection::new(Arc::new(config), ServerName::try_from(hostname.to_owned()).unwrap()).unwrap();
    for _ in 0..8 {
        let mut wire = Vec::new();
        client.write_tls(&mut wire).unwrap();
        server.read_tls(&mut wire.as_slice()).unwrap();
        server.process_new_packets()?;
        wire.clear();
        server.write_tls(&mut wire).unwrap();
        client.read_tls(&mut wire.as_slice()).unwrap();
        client.process_new_packets()?;
        if !client.is_handshaking() && !server.is_handshaking() {
            return Ok(());
        }
    }
    panic!("in-memory TLS handshake did not complete");
}

#[test]
fn verified_tls_enforces_roots_hostname_and_optional_sans() {
    let certificate = rcgen::generate_simple_self_signed(vec!["backend.example.com".to_string()]).unwrap();
    let resolved = ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::Custom(vec![certificate.cert.pem().into_bytes()]),
        client_identity: None,
        san: None,
    };
    let build = |sans| build_verified_outbound_tls(&resolved, sans, "Test").unwrap();
    assert!(handshake(build(vec![]), &certificate, "backend.example.com").is_ok());
    assert!(handshake(build(vec![]), &certificate, "wrong.example.com").is_err());
    let san = |hostname: &str| SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some(hostname.to_string()),
        uri: None,
    };
    assert!(handshake(
        build(vec![san("backend.example.com")]),
        &certificate,
        "backend.example.com"
    )
    .is_ok());
    assert!(handshake(
        build(vec![san("wrong.example.com")]),
        &certificate,
        "backend.example.com"
    )
    .is_err());
    let other = rcgen::generate_simple_self_signed(vec!["backend.example.com".to_string()]).unwrap();
    assert!(handshake(build(vec![]), &other, "backend.example.com").is_err());
}

#[test]
fn verified_tls_rejects_mismatched_client_identity() {
    let certificate = rcgen::generate_simple_self_signed(vec!["client.example.com".to_string()]).unwrap();
    let other = rcgen::generate_simple_self_signed(vec!["client.example.com".to_string()]).unwrap();
    let mut resolved = ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::System,
        client_identity: Some((
            certificate.cert.pem().into_bytes(),
            other.signing_key.serialize_pem().into_bytes().into(),
        )),
        san: None,
    };
    let error = build_verified_outbound_tls(&resolved, vec![], "Test").unwrap_err();
    assert!(error.contains("client auth cert error"));
    resolved.client_identity = Some((
        certificate.cert.pem().into_bytes(),
        certificate.signing_key.serialize_pem().into_bytes().into(),
    ));
    let config = build_verified_outbound_tls(&resolved, vec![], "Test").unwrap();
    assert!(config.client_auth_cert_resolver.has_certs());
}

#[test]
fn verified_tls_rejects_each_zero_certificate_ca_bundle() {
    let certificate = rcgen::generate_simple_self_signed(vec!["backend.example.com".to_string()]).unwrap();

    for invalid in [b"".as_slice(), b" \n\t".as_slice(), b"not-a-pem".as_slice()] {
        for invalid_first in [false, true] {
            let mut bundles = vec![certificate.cert.pem().into_bytes(), invalid.to_vec()];
            if invalid_first {
                bundles.swap(0, 1);
            }
            let resolved = ResolvedOutboundTls {
                verify: true,
                ca: EffectiveCa::Custom(bundles),
                client_identity: None,
                san: None,
            };

            build_verified_outbound_tls(&resolved, vec![], "Test")
                .expect_err("every declared verified TLS CA bundle must contain a certificate");
        }
    }
}

#[test]
fn verified_tls_accepts_multiple_valid_bundles_and_certificates() {
    let certificate_a = rcgen::generate_simple_self_signed(vec!["backend-a.example.com".to_string()]).unwrap();
    let certificate_b = rcgen::generate_simple_self_signed(vec!["backend-b.example.com".to_string()]).unwrap();
    let pem_a = certificate_a.cert.pem();
    let pem_b = certificate_b.cert.pem();

    let multiple_bundles = ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::Custom(vec![pem_a.clone().into_bytes(), pem_b.clone().into_bytes()]),
        client_identity: None,
        san: None,
    };
    assert!(build_verified_outbound_tls(&multiple_bundles, vec![], "Test").is_ok());

    let multiple_certificates = ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::Custom(vec![format!("{pem_a}\n{pem_b}\n").into_bytes()]),
        client_identity: None,
        san: None,
    };
    assert!(build_verified_outbound_tls(&multiple_certificates, vec![], "Test").is_ok());
}

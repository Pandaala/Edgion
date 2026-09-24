use super::*;

#[test]
fn every_resolved_ca_bundle_must_be_valid_without_controller_error_marker() {
    let ca = rcgen::generate_simple_self_signed(vec!["probe.example.com".to_owned()]).unwrap();
    let mut config = config(&ca.cert);
    config
        .tls
        .as_mut()
        .unwrap()
        .validation
        .as_mut()
        .unwrap()
        .ca_certificate_refs
        .as_mut()
        .unwrap()
        .push(serde_json::from_value(serde_json::json!({"kind": "Secret", "name": "second-ca"})).unwrap());
    for invalid in [
        String::new(),
        "not a PEM certificate".to_owned(),
        "-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----".to_owned(),
    ] {
        config.resolved_ca_certificates = Some(vec![
            secret(&[("ca.crt", ca.cert.pem())]),
            secret(&[("ca.crt", invalid)]),
        ]);
        assert!(config.resolved_tls_error.is_none());
        assert!(matches!(
            EncryptedProbeTransport::build(&config, "default"),
            Err(ProbeError::InvalidTlsConfig)
        ));
    }
}

fn secret(entries: &[(&str, String)]) -> k8s_openapi::api::core::v1::Secret {
    k8s_openapi::api::core::v1::Secret {
        type_: Some("kubernetes.io/tls".to_string()),
        data: Some(
            entries
                .iter()
                .map(|(key, value)| (key.to_string(), k8s_openapi::ByteString(value.as_bytes().to_vec())))
                .collect(),
        ),
        ..Default::default()
    }
}

fn config(ca: &rcgen::Certificate) -> ActiveHealthCheckConfig {
    let mut config: ActiveHealthCheckConfig = serde_json::from_value(serde_json::json!({
        "type": "https",
        "tls": { "validation": {
            "hostname": "probe.example.com",
            "caCertificateRefs": [{"kind": "Secret", "name": "probe-ca"}]
        }}
    }))
    .unwrap();
    config.resolved_ca_certificates = Some(vec![secret(&[("ca.crt", ca.pem())])]);
    config
}

fn handshake(
    transport: EncryptedProbeTransport,
    certificate: &rcgen::Certificate,
    key: &rcgen::KeyPair,
    client_ca: Option<&rcgen::Certificate>,
) -> Result<(), rustls::Error> {
    use rustls::pki_types::{pem::PemObject, PrivateKeyDer, ServerName};
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .unwrap();
    let builder = if let Some(ca) = client_ca {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.der().clone()).unwrap();
        builder.with_client_cert_verifier(
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .unwrap(),
        )
    } else {
        builder.with_no_client_auth()
    };
    let server_config = builder
        .with_single_cert(
            vec![certificate.der().clone()],
            PrivateKeyDer::from_pem_slice(key.serialize_pem().as_bytes()).unwrap(),
        )
        .unwrap();
    let mut server = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
    let mut client = rustls::ClientConnection::new(
        Arc::new(transport.tls),
        ServerName::try_from(transport.hostname).unwrap(),
    )
    .unwrap();
    for _ in 0..8 {
        let mut bytes = Vec::new();
        client.write_tls(&mut bytes).unwrap();
        server.read_tls(&mut bytes.as_slice()).unwrap();
        server.process_new_packets()?;
        bytes.clear();
        server.write_tls(&mut bytes).unwrap();
        client.read_tls(&mut bytes.as_slice()).unwrap();
        client.process_new_packets()?;
        if !client.is_handshaking() && !server.is_handshaking() {
            return Ok(());
        }
    }
    panic!("in-memory health TLS handshake did not complete");
}

#[test]
fn health_tls_rejects_expired_peer_even_with_trusted_certificate() {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["probe.example.com".to_string()]).unwrap();
    let valid = params.self_signed(&key).unwrap();
    assert!(handshake(
        EncryptedProbeTransport::build(&config(&valid), "default").unwrap(),
        &valid,
        &key,
        None
    )
    .is_ok());
    params.not_before = rcgen::date_time_ymd(2000, 1, 1);
    params.not_after = rcgen::date_time_ymd(2001, 1, 1);
    let expired = params.self_signed(&key).unwrap();
    assert!(matches!(
        handshake(
            EncryptedProbeTransport::build(&config(&expired), "default").unwrap(),
            &expired,
            &key,
            None
        ),
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::Expired | rustls::CertificateError::ExpiredContext { .. }
        ))
    ));
}

#[test]
fn health_tls_requires_valid_client_identity_for_mtls() {
    let server = rcgen::generate_simple_self_signed(vec!["probe.example.com".to_owned()]).unwrap();
    let client_ca_key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let client_ca = params.self_signed(&client_ca_key).unwrap();
    let issuer = rcgen::Issuer::from_ca_cert_pem(&client_ca.pem(), client_ca_key).unwrap();
    let client_key = rcgen::KeyPair::generate().unwrap();
    let mut client_params = rcgen::CertificateParams::new(vec!["client.example.com".to_owned()]).unwrap();
    client_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let client = client_params.signed_by(&client_key, &issuer).unwrap();
    let mut config = config(&server.cert);
    assert!(handshake(
        EncryptedProbeTransport::build(&config, "default").unwrap(),
        &server.cert,
        &server.signing_key,
        Some(&client_ca)
    )
    .is_err());
    config.tls.as_mut().unwrap().client_certificate_ref =
        Some(serde_json::from_value(serde_json::json!({"name": "probe-client"})).unwrap());
    assert!(matches!(
        EncryptedProbeTransport::build(&config, "default"),
        Err(ProbeError::InvalidTlsConfig)
    ));
    config.resolved_client_certificate = Some(secret(&[
        ("tls.crt", client.pem()),
        ("tls.key", client_key.serialize_pem()),
    ]));
    for protocol in [
        edgion_resources::resources::health_check::HealthCheckType::Https,
        edgion_resources::resources::health_check::HealthCheckType::Grpcs,
    ] {
        config.r#type = protocol;
        assert!(handshake(
            EncryptedProbeTransport::build(&config, "default").unwrap(),
            &server.cert,
            &server.signing_key,
            Some(&client_ca)
        )
        .is_ok());
    }
    for (certificate, private_key) in [
        (
            format!("{}\ninvalid trailing material", client.pem()),
            client_key.serialize_pem(),
        ),
        (
            client.pem(),
            format!("{}\ninvalid trailing material", client_key.serialize_pem()),
        ),
    ] {
        config.resolved_client_certificate = Some(secret(&[("tls.crt", certificate), ("tls.key", private_key)]));
        assert!(config.resolved_tls_error.is_none());
        assert!(matches!(
            EncryptedProbeTransport::build(&config, "default"),
            Err(ProbeError::InvalidTlsConfig)
        ));
    }
    let wrong_key = rcgen::KeyPair::generate().unwrap();
    config.resolved_client_certificate = Some(secret(&[
        ("tls.crt", client.pem()),
        ("tls.key", wrong_key.serialize_pem()),
    ]));
    assert!(matches!(
        EncryptedProbeTransport::build(&config, "default"),
        Err(ProbeError::InvalidTlsConfig)
    ));
}

#[test]
fn encrypted_hostname_cannot_escape_endpoint_pin() {
    let address = "10.0.0.1:8443".parse().unwrap();
    for hostname in ["127.0.0.1", "2130706433", "0x7f000001", "0177.0.0.1", "[::1]"] {
        assert!(matches!(
            encrypted_url(hostname, address, "/"),
            Err(ProbeError::InvalidTlsConfig)
        ));
    }
    let url = encrypted_url(
        "probe.example.com",
        address,
        "//elsewhere.example/p%2Fa?q=value#fragment",
    )
    .unwrap();
    assert_eq!(url.host_str(), Some("probe.example.com"));
    assert_eq!(url.port(), Some(8443));
    assert_eq!(url.path(), "//elsewhere.example/p%2Fa");
    assert_eq!(url.query(), Some("q=value%23fragment"));
    assert!(url.fragment().is_none());
}

#[test]
fn missing_tls_material_fails_closed_without_building_plaintext() {
    let config = ActiveHealthCheckConfig {
        r#type: edgion_resources::resources::health_check::HealthCheckType::Https,
        ..Default::default()
    };
    assert!(matches!(
        EncryptedProbeTransport::build(&config, "default"),
        Err(ProbeError::InvalidTlsConfig)
    ));
}

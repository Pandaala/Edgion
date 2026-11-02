use super::*;

fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

// Minimal in-test CA/leaf generator (mirrors edgion-controller's conf_sync_ca::Ca,
// duplicated here because edgion-common cannot depend on edgion-controller).
fn gen_ca_and_leaf(leaf_days: i64) -> (String, String, String) {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair};
    let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut cp = CertificateParams::default();
    cp.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    cp.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "test-ca");
        dn
    };
    let ca_cert = cp.self_signed(&ca_key).unwrap();
    let issuer = Issuer::from_ca_cert_pem(&ca_cert.pem(), ca_key).unwrap();
    let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut lp = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    lp.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(5);
    lp.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(leaf_days);
    let leaf = lp.signed_by(&leaf_key, &issuer).unwrap();
    (leaf.pem(), leaf_key.serialize_pem(), ca_cert.pem())
}

#[test]
fn accepts_valid_identity() {
    install_test_crypto_provider();
    let (cert, key, ca) = gen_ca_and_leaf(30);
    validate_identity(cert.as_bytes(), key.as_bytes(), ca.as_bytes()).unwrap();
}

#[test]
fn rejects_expired_leaf() {
    install_test_crypto_provider();
    let (cert, key, ca) = gen_ca_and_leaf(-1);
    assert!(validate_identity(cert.as_bytes(), key.as_bytes(), ca.as_bytes()).is_err());
}

#[test]
fn rejects_wrong_ca() {
    install_test_crypto_provider();
    let (cert, key, _) = gen_ca_and_leaf(30);
    let (_, _, other_ca) = gen_ca_and_leaf(30);
    assert!(validate_identity(cert.as_bytes(), key.as_bytes(), other_ca.as_bytes()).is_err());
}

#[test]
fn rejects_mismatched_key() {
    install_test_crypto_provider();
    let (cert, _, ca) = gen_ca_and_leaf(30);
    let (_, other_key, _) = gen_ca_and_leaf(30);
    assert!(validate_identity(cert.as_bytes(), other_key.as_bytes(), ca.as_bytes()).is_err());
}

#[test]
fn rejects_garbage() {
    install_test_crypto_provider();
    assert!(validate_identity(b"nope", b"nope", b"nope").is_err());
}

#[test]
fn normal_chains_accept_but_wrong_leaf_and_ca_labels_reject_with_fixed_errors() {
    install_test_crypto_provider();
    let (cert, key, ca) = gen_ca_and_leaf(30);
    let chain = format!("{cert}{ca}");
    validate_identity(chain.as_bytes(), key.as_bytes(), ca.as_bytes()).unwrap();
    for label in ["PUBLIC KEY", "PRIVATE_MARKER"] {
        let disguised_leaf = cert.replace("CERTIFICATE", label);
        let leaf_input = format!("{disguised_leaf}{cert}");
        assert_eq!(
            validate_identity(leaf_input.as_bytes(), key.as_bytes(), ca.as_bytes())
                .unwrap_err()
                .to_string(),
            "leaf cert PEM must begin with a CERTIFICATE block"
        );
        let disguised_ca = ca.replace("CERTIFICATE", label);
        let ca_input = format!("{disguised_ca}{ca}");
        assert_eq!(
            validate_identity(cert.as_bytes(), key.as_bytes(), ca_input.as_bytes())
                .unwrap_err()
                .to_string(),
            "CA cert PEM must begin with a CERTIFICATE block"
        );
    }
    // Initial lazy client construction keeps its separate first-CERTIFICATE rule.
    let mixed = format!("{}{cert}", cert.replace("CERTIFICATE", "PUBLIC KEY"));
    let (_, validity) = client_tls_from_pems(mixed.into_bytes(), key.into_bytes(), ca.into_bytes()).unwrap();
    assert_eq!(validity, parse_leaf_validity(cert).unwrap());
}

#[test]
fn private_key_loading_skips_other_sections_and_selects_the_first_key() {
    install_test_crypto_provider();
    let (cert, key, ca) = gen_ca_and_leaf(30);
    let (_, other_key, _) = gen_ca_and_leaf(30);
    let bundle = format!("{cert}{key}{other_key}");
    validate_identity(cert.as_bytes(), bundle.as_bytes(), ca.as_bytes()).unwrap();
    let bundle = format!("{cert}{other_key}{key}");
    assert_eq!(
        validate_identity(cert.as_bytes(), bundle.as_bytes(), ca.as_bytes())
            .unwrap_err()
            .to_string(),
        "private key does not match the leaf certificate public key"
    );
}

#[test]
fn missing_or_malformed_private_keys_have_fixed_errors() {
    install_test_crypto_provider();
    let (cert, _, ca) = gen_ca_and_leaf(30);
    assert_eq!(
        validate_identity(cert.as_bytes(), cert.as_bytes(), ca.as_bytes())
            .unwrap_err()
            .to_string(),
        "no private key found in PEM"
    );
    for key in [
        "-----BEGIN PRIVATE KEY SECRET_MARKER\n",
        "-----BEGIN PRIVATE KEY-----\nnot-base64!\n-----END PRIVATE KEY-----\n",
    ] {
        assert_eq!(
            validate_identity(cert.as_bytes(), key.as_bytes(), ca.as_bytes())
                .unwrap_err()
                .to_string(),
            "private key PEM parse: invalid PEM"
        );
    }
}

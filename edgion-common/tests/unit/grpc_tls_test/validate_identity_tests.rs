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

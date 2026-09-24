use super::*;

// Minimal in-test CA/leaf generator (copied verbatim from edgion-common's
// `validate_identity` tests — edgion-gateway cannot reach that test module).
fn gen_material(_dir: &std::path::Path) -> (String, String, String) {
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
    lp.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(30);
    let leaf = lp.signed_by(&leaf_key, &issuer).unwrap();
    (leaf.pem(), leaf_key.serialize_pem(), ca_cert.pem())
}

#[tokio::test]
async fn reload_decision_validates_before_applying() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key, ca) = gen_material(dir.path());
    std::fs::write(dir.path().join("client.crt"), &cert).unwrap();
    std::fs::write(dir.path().join("client.key"), &key).unwrap();
    std::fs::write(dir.path().join("ca.crt"), &ca).unwrap();
    let paths = CertPaths {
        cert: dir.path().join("client.crt"),
        key: dir.path().join("client.key"),
        ca: dir.path().join("ca.crt"),
    };
    assert!(read_and_validate(&paths).await.is_ok());
    std::fs::write(dir.path().join("client.crt"), "garbage").unwrap();
    assert!(read_and_validate(&paths).await.is_err());
}

#[test]
fn restat_detects_mtime_or_len_change() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f");
    std::fs::write(&p, "aaa").unwrap();
    let s1 = stat_signature(std::slice::from_ref(&p)).unwrap();
    std::fs::write(&p, "aaaa").unwrap();
    let s2 = stat_signature(std::slice::from_ref(&p)).unwrap();
    assert_ne!(s1, s2);
}

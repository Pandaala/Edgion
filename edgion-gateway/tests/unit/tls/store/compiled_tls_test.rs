fn make_self_signed_pem(cn: &str) -> String {
    use rcgen::{CertificateParams, KeyPair};
    let key = KeyPair::generate().expect("rcgen key generation failed");
    let params = CertificateParams::new(vec![cn.to_string()]).expect("rcgen params failed");
    params.self_signed(&key).expect("rcgen self-sign failed").pem()
}

#[test]
fn ca_bundle_loads_all_pem_blocks() {
    use super::add_ca_bundle_to_store;
    use pingora_core::tls::x509::store::X509StoreBuilder;
    use pingora_core::tls::x509::X509;

    // Realistic shape: a root + two intermediates concatenated into ca.crt.
    let bundle = format!(
        "{}{}{}",
        make_self_signed_pem("ca-root"),
        make_self_signed_pem("ca-intermediate-1"),
        make_self_signed_pem("ca-intermediate-2"),
    );

    // Regression guard: the old `X509::from_pem` path saw only the first
    // block; the helper must load all three.
    assert_eq!(X509::stack_from_pem(bundle.as_bytes()).unwrap().len(), 3);

    let mut builder = X509StoreBuilder::new().expect("store builder");
    let added = add_ca_bundle_to_store(&mut builder, bundle.as_bytes()).expect("add bundle");
    assert_eq!(added, 3, "all three CA certs must be added to the store");
}

#[test]
fn ca_bundle_rejects_empty_or_garbage() {
    use super::add_ca_bundle_to_store;
    use pingora_core::tls::x509::store::X509StoreBuilder;

    let mut builder = X509StoreBuilder::new().expect("store builder");
    assert!(add_ca_bundle_to_store(&mut builder, b"").is_err());
    assert!(add_ca_bundle_to_store(&mut builder, b"not a pem at all").is_err());
}

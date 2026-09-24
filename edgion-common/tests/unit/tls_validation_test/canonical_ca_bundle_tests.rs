use super::{
    parse_canonical_ca_bundle, parse_strict_private_key_pem, CaBundleError, MAX_CA_BUNDLE_BYTES_PER_REF,
    MAX_CA_CERTIFICATES_PER_REF,
};

fn certificate_and_key() -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["ca.example.com".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let cert = params.self_signed(&key).unwrap().pem();
    (cert, key.serialize_pem())
}

#[test]
fn accepts_only_complete_certificate_blocks_and_canonicalizes_duplicates() {
    let (cert, _) = certificate_and_key();
    let bundle = format!("\n{cert}\n{cert}\n");
    let parsed = parse_canonical_ca_bundle(bundle.as_bytes()).unwrap();
    assert_eq!(parsed.certificates_der.len(), 1);
}

#[test]
fn rejects_trailing_garbage_private_key_and_truncated_certificate() {
    let (cert, key) = certificate_and_key();
    assert_eq!(
        parse_canonical_ca_bundle(format!("{cert}garbage").as_bytes()),
        Err(CaBundleError::UnexpectedContent)
    );
    assert_eq!(
        parse_canonical_ca_bundle(format!("{cert}{key}").as_bytes()),
        Err(CaBundleError::UnexpectedContent)
    );
    let truncated = cert.replace("-----END CERTIFICATE-----", "");
    assert_eq!(
        parse_canonical_ca_bundle(truncated.as_bytes()),
        Err(CaBundleError::TruncatedPem)
    );
}

#[test]
fn rejects_empty_non_certificate_and_invalid_utf8_inputs() {
    assert_eq!(parse_canonical_ca_bundle(b" \n\t"), Err(CaBundleError::Empty));
    assert_eq!(
        parse_canonical_ca_bundle(b"-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n"),
        Err(CaBundleError::UnexpectedContent)
    );
    assert_eq!(parse_canonical_ca_bundle(&[0xff]), Err(CaBundleError::InvalidUtf8));
}

#[test]
fn enforces_per_reference_byte_limit_at_exact_boundary() {
    let (cert, _) = certificate_and_key();
    let mut exact = vec![b' '; MAX_CA_BUNDLE_BYTES_PER_REF - cert.len()];
    exact.extend_from_slice(cert.as_bytes());
    assert!(parse_canonical_ca_bundle(&exact).is_ok());
    exact.push(b' ');
    assert_eq!(parse_canonical_ca_bundle(&exact), Err(CaBundleError::TooLarge));
}

#[test]
fn enforces_unique_certificate_count_before_push() {
    let mut bundle = String::new();
    for index in 0..=MAX_CA_CERTIFICATES_PER_REF {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec![format!("ca-{index}.example.com")]).unwrap();
        bundle.push_str(&params.self_signed(&key).unwrap().pem());
    }
    assert_eq!(
        parse_canonical_ca_bundle(bundle.as_bytes()),
        Err(CaBundleError::TooManyCertificates)
    );
}

#[test]
fn strict_private_key_rejects_trailing_certificate_content() {
    let (cert, key) = certificate_and_key();
    assert!(parse_strict_private_key_pem(key.as_bytes()).is_ok());
    assert!(parse_strict_private_key_pem(format!("{key}{cert}").as_bytes()).is_err());
}

use super::*;
use boring::ssl::{Ssl, SslContext, SslMethod};

#[test]
fn cipher_presence_and_override_resolution_preserve_empty_and_rejection() {
    let absent = CipherPolicy::compile(None);
    let empty = CipherPolicy::compile(Some(&[]));
    let invalid = CipherPolicy::compile(Some(&["DEFAULT\0bad".into()]));
    let explicit = CipherPolicy::compile(Some(&["HIGH".into(), "!aNULL".into()]));
    assert!(matches!(absent, CipherPolicy::Inherit));
    assert!(matches!(empty, CipherPolicy::BackendDefault));
    assert!(matches!(invalid, CipherPolicy::Reject(_)));
    assert!(matches!(&explicit, CipherPolicy::Explicit(value) if value.to_bytes() == b"HIGH:!aNULL"));
    assert!(std::ptr::eq(invalid.resolve(Some(&explicit)), &explicit));
    assert!(std::ptr::eq(invalid.resolve(Some(&empty)), &empty));
    assert!(std::ptr::eq(invalid.resolve(Some(&absent)), &invalid));
}

#[test]
fn effective_backend_validation_and_fips_curve_rejection_are_independent() {
    let context = SslContext::builder(SslMethod::tls()).unwrap().build();
    let mut ssl = Ssl::new(&context).unwrap();
    let invalid = DownstreamTlsPolicy::compile(Some(&DownstreamTlsConfig {
        ciphers: Some(vec!["NOT-A-CIPHER".into()]),
        ..Default::default()
    }));
    assert_eq!(
        invalid.apply(&mut ssl, None, None, false),
        Err("cipher config rejected")
    );
    let local = CipherPolicy::compile(Some(&["ECDHE-RSA-AES128-GCM-SHA256".into()]));
    invalid.apply(&mut ssl, None, Some(&local), true).unwrap();
    let incompatible = DownstreamTlsPolicy::compile(Some(&DownstreamTlsConfig {
        ecdh_curves: Some(vec!["X25519".into()]),
        ..Default::default()
    }));
    assert_eq!(
        incompatible.apply(&mut ssl, None, Some(&local), true),
        Err("ECDH curves conflict with FIPS policy")
    );
    let empty = CipherPolicy::compile(Some(&[]));
    assert_eq!(
        incompatible.apply(&mut ssl, None, Some(&empty), true),
        Err("ECDH curves conflict with FIPS policy")
    );
}

use super::*;
use crate::ctx::ClientCertInfo;
use edgion_common::allowed_san::PresentedSan;
use edgion_resources::resources::edgion_tls::AllowedSanType;

#[test]
fn test_matches_pattern_exact() {
    use edgion_algo::matcher::rfc6125_hostname::matches;
    assert!(matches("example.com", "example.com"));
    assert!(!matches("other.com", "example.com"));
}

#[test]
fn test_matches_pattern_wildcard() {
    use edgion_algo::matcher::rfc6125_hostname::matches;
    assert!(matches("*.example.com", "sub.example.com"));
    assert!(!matches("*.example.com", "example.com"));
    assert!(!matches("*.example.com", "sub.sub.example.com"));
}

#[test]
fn test_validate_san_whitelist_empty() {
    assert!(validate_san_whitelist(&[PresentedSan::dns("test.example.com")], &[]));
}

#[test]
fn test_validate_san_whitelist_match() {
    let presented = [PresentedSan::dns("test.example.com"), PresentedSan::dns("other.com")];
    assert!(validate_san_whitelist(
        &presented,
        &[AllowedSan::exact(AllowedSanType::Dns, "test.example.com")]
    ));
    assert!(validate_san_whitelist(
        &presented,
        &[AllowedSan::exact(AllowedSanType::Dns, "other.com")]
    ));
    assert!(validate_san_whitelist(
        &presented,
        &[AllowedSan::exact(AllowedSanType::Dns, "*.example.com")]
    ));
}

#[test]
fn test_validate_san_whitelist_no_match() {
    assert!(!validate_san_whitelist(
        &[PresentedSan::dns("test.example.com")],
        &[AllowedSan::exact(AllowedSanType::Dns, "other.com")]
    ));
}

#[test]
fn test_validate_cn_whitelist() {
    let cert_info = ClientCertInfo {
        subject: "CN=TestUser".to_string(),
        sans: vec![],
        cn: Some("TestUser".to_string()),
        fingerprint: "abc123".to_string(),
    };

    assert!(validate_cn_whitelist(&cert_info, &[])); // Empty whitelist
    assert!(validate_cn_whitelist(&cert_info, &["TestUser".to_string()]));
    assert!(!validate_cn_whitelist(&cert_info, &["OtherUser".to_string()]));
    assert!(!validate_cn_whitelist(&cert_info, &["testuser".to_string()]));
    assert!(!validate_cn_whitelist(
        &ClientCertInfo {
            cn: Some("client.example.com".to_string()),
            ..cert_info
        },
        &["*.example.com".to_string()]
    ));
}

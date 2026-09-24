use super::*;

#[test]
fn test_default_config() {
    let cfg = HeaderCertAuthConfig::default();
    assert_eq!(cfg.mode, CertSourceMode::Header);
    assert_eq!(cfg.certificate_header_name, "X-Client-Cert");
    assert_eq!(cfg.error_status, 401);
    assert!(cfg.hide_credentials);
}

#[test]
fn test_detect_validation_error_requires_ca_for_header_mode() {
    let cfg = HeaderCertAuthConfig::default();
    let err = cfg.detect_validation_error();
    assert!(err.is_some());
    assert!(err.unwrap().contains("caSecretRefs"));
}

#[test]
fn test_detect_validation_error_connection_mode_without_ca_ok() {
    let cfg = HeaderCertAuthConfig {
        mode: CertSourceMode::Connection,
        ..Default::default()
    };
    assert!(cfg.detect_validation_error().is_none());
}

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

#[test]
fn configured_header_names_require_http_tokens_without_normalization() {
    for mode in ["Header", "Connection"] {
        for field in [
            "certificateHeaderName",
            "consumerHeader",
            "dnHeader",
            "sanHeader",
            "fingerprintHeader",
        ] {
            for name in [
                "private name",
                "private\tname",
                "private:name",
                "private-é",
                "X-Mixed!#$%&'*+-.^_`|~09",
            ] {
                let mut value = serde_json::json!({"mode": mode, "caSecretRefs": [{"name": "ca"}]});
                let path = if field == "certificateHeaderName" {
                    value[field] = name.into();
                    field.to_string()
                } else {
                    value["upstreamHeaders"] = serde_json::json!({field: name});
                    format!("upstreamHeaders.{field}")
                };
                let config: HeaderCertAuthConfig = serde_json::from_value(value).unwrap();
                let before = serde_json::to_value(&config).unwrap();
                if name.starts_with("X-Mixed") {
                    assert_eq!(config.detect_validation_error(), None);
                } else {
                    assert_eq!(
                        config.detect_validation_error(),
                        Some(format!("{path} must be a valid HTTP header name"))
                    );
                }
                assert_eq!(serde_json::to_value(&config).unwrap(), before);
            }
        }
    }
}

#[test]
fn header_name_empty_and_control_checks_keep_cross_field_precedence() {
    let mut config = HeaderCertAuthConfig {
        certificate_header_name: "private name".into(),
        ..Default::default()
    };
    config.upstream_headers.consumer_header = " ".into();
    assert_eq!(
        config.detect_validation_error().as_deref(),
        Some("upstreamHeaders entries cannot be empty")
    );
    config.upstream_headers.consumer_header = "private:name".into();
    for control in ["private\rname", "private\nname", "private\0name"] {
        config.upstream_headers.fingerprint_header = control.into();
        assert_eq!(
            config.detect_validation_error().as_deref(),
            Some("upstreamHeaders entries contain invalid control characters")
        );
        config.certificate_header_name = control.into();
        assert_eq!(
            config.detect_validation_error().as_deref(),
            Some("certificateHeaderName contains invalid control characters")
        );
        config.certificate_header_name = "private name".into();
    }
    config.certificate_header_name = String::new();
    assert_eq!(
        config.detect_validation_error().as_deref(),
        Some("certificateHeaderName cannot be empty")
    );
}

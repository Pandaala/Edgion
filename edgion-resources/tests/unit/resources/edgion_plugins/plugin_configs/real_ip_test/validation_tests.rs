use super::*;

#[test]
fn test_plugin_validation_empty_trusted_ips() {
    let config = RealIpConfig::default();
    let err = config.validation_error();
    assert!(err.is_some());
    assert_eq!(err.unwrap(), "'trusted_ips' must not be empty");
}

#[test]
fn test_plugin_validation_empty_real_ip_header() {
    let config = RealIpConfig {
        trusted_ips: vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }],
        real_ip_header: "   ".to_string(),
        ..Default::default()
    };
    let err = config.validation_error();
    assert!(err.is_some());
    assert_eq!(err.unwrap(), "'real_ip_header' must not be empty");
}

#[test]
fn test_plugin_validation_all_invalid_ips() {
    let config = RealIpConfig {
        trusted_ips: vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["not-an-ip".to_string(), "999.999.999.999".to_string()],
        }],
        ..Default::default()
    };
    let err = config.validation_error();
    assert!(err.is_some());
    let msg = err.unwrap();
    assert!(msg.starts_with("trusted_ips:"), "unexpected message: {msg}");
    assert!(msg.contains("Invalid IP"), "unexpected message: {msg}");
}

fn config_with_header(name: &str) -> RealIpConfig {
    RealIpConfig {
        trusted_ips: vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".into()],
        }],
        real_ip_header: name.to_string(),
        ..Default::default()
    }
}

#[test]
fn test_plugin_validation_rejects_non_token_header_names() {
    for bad in ["X Forwarded For", "X-Forwarded-For:", " X-Real-IP ", "head(er)"] {
        let err = config_with_header(bad).validation_error();
        assert_eq!(
            err.as_deref(),
            Some("'real_ip_header' must be a valid HTTP header name"),
            "should reject {bad:?}"
        );
    }
}

#[test]
fn test_plugin_validation_rejects_oversized_header_name() {
    let oversized = "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize + 1);
    assert_eq!(
        config_with_header(&oversized).validation_error().as_deref(),
        Some("'real_ip_header' must be a valid HTTP header name")
    );
    let at_bound = "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize);
    assert!(config_with_header(&at_bound).validation_error().is_none());
}

#[test]
fn test_plugin_validation_accepts_default_and_custom_header_names() {
    for ok in ["X-Forwarded-For", "CF-Connecting-IP", "True-Client-IP", "x_real.ip"] {
        assert!(
            config_with_header(ok).validation_error().is_none(),
            "should accept {ok:?}"
        );
    }
}

#[test]
fn test_plugin_validation_valid_config() {
    let config = RealIpConfig {
        trusted_ips: vec![IpGroup {
            name: "default".into(),
            description: None,
            cidrs: vec!["10.0.0.0/8".to_string(), "192.168.1.1".to_string()],
        }],
        real_ip_header: "X-Forwarded-For".to_string(),
        ..Default::default()
    };
    assert!(config.validation_error().is_none());
}

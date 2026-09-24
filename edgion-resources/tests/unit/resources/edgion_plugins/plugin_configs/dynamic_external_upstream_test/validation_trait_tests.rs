use super::*;
use crate::resources::edgion_plugins::PluginValidation;

#[test]
fn validation_error_returns_field_error_without_validate_call() {
    // Default config has an empty domain_map, which is the first field error
    // (after regex check passes since extract is None).
    let config = DynamicExternalUpstreamConfig::default();
    let result = config.validation_error();
    assert_eq!(result, Some("domainMap cannot be empty".to_string()));
}

#[test]
fn validation_error_detects_invalid_regex() {
    let config = DynamicExternalUpstreamConfig {
        extract: Some(ExtUpstreamExtract {
            regex: "[invalid".to_string(),
            group: 1,
        }),
        ..Default::default()
    };
    let result = config.validation_error();
    assert!(
        result.as_deref().unwrap_or("").starts_with("Invalid extract regex:"),
        "expected regex error, got: {:?}",
        result
    );
}

#[test]
fn validation_error_none_for_valid_config() {
    let mut config = DynamicExternalUpstreamConfig::default();
    config.domain_map.insert(
        "us".to_string(),
        DomainTarget {
            domain: "api-us.example.com".to_string(),
            port: Some(443),
            tls: true,
            override_host: None,
            sni: None,
        },
    );
    assert_eq!(config.validation_error(), None);
}

#[test]
fn validation_error_detects_empty_domain() {
    let mut config = DynamicExternalUpstreamConfig::default();
    config.domain_map.insert(
        "us".to_string(),
        DomainTarget {
            domain: "".to_string(),
            port: None,
            tls: true,
            override_host: None,
            sni: None,
        },
    );
    let result = config.validation_error();
    assert_eq!(result, Some("domainMap['us']: domain cannot be empty".to_string()));
}

#[test]
fn validation_error_detects_zero_port() {
    let mut config = DynamicExternalUpstreamConfig::default();
    config.domain_map.insert(
        "eu".to_string(),
        DomainTarget {
            domain: "api.example.com".to_string(),
            port: Some(0),
            tls: true,
            override_host: None,
            sni: None,
        },
    );
    let result = config.validation_error();
    assert_eq!(result, Some("domainMap['eu']: port cannot be 0".to_string()));
}

#[test]
fn validation_error_detects_invalid_domain_char() {
    let mut config = DynamicExternalUpstreamConfig::default();
    config.domain_map.insert(
        "bad".to_string(),
        DomainTarget {
            domain: "api example.com".to_string(), // space is invalid
            port: None,
            tls: true,
            override_host: None,
            sni: None,
        },
    );
    let result = config.validation_error();
    assert_eq!(
        result,
        Some("domainMap['bad']: domain contains invalid character ' '".to_string())
    );
}

fn target_with(domain: &str) -> DomainTarget {
    DomainTarget {
        domain: domain.to_string(),
        port: None,
        tls: true,
        override_host: None,
        sni: None,
    }
}

fn config_with(target: DomainTarget) -> DynamicExternalUpstreamConfig {
    let mut config = DynamicExternalUpstreamConfig::default();
    config.domain_map.insert("us".to_string(), target);
    config
}

#[test]
fn validation_error_none_when_sni_and_override_host_are_omitted() {
    assert_eq!(config_with(target_with("api-us.example.com")).validation_error(), None);
}

#[test]
fn validation_error_none_for_explicit_valid_sni_and_supported_name_forms() {
    for sni in ["api-b.example.com", "localhost", "127.0.0.1", "us-west.internal"] {
        let mut target = target_with("api-us.example.com");
        target.sni = Some(sni.to_string());
        assert_eq!(
            config_with(target).validation_error(),
            None,
            "supported sni form must remain valid: {sni}"
        );
    }
}

#[test]
fn validation_error_rejects_explicit_empty_sni() {
    let mut target = target_with("api-us.example.com");
    target.sni = Some(String::new());
    assert_eq!(config_with(target).validation_error(), Some(EMPTY_SNI.to_string()));
}

#[test]
fn validation_error_rejects_sni_with_invalid_characters() {
    let mut target = target_with("api-us.example.com");
    target.sni = Some("api example.com".to_string());
    assert_eq!(config_with(target).validation_error(), Some(INVALID_SNI.to_string()));
}

#[test]
fn validation_error_none_for_explicit_valid_override_host() {
    for host in [
        "localhost",
        "us-west.internal",
        "api.example.com:8443",
        "[::1]",
        "[::1]:443",
    ] {
        let mut target = target_with("api-us.example.com");
        target.override_host = Some(host.to_string());
        assert_eq!(
            config_with(target).validation_error(),
            None,
            "supported overrideHost form must remain valid: {host}"
        );
    }
}

#[test]
fn validation_error_rejects_explicit_empty_override_host() {
    let mut target = target_with("api-us.example.com");
    target.override_host = Some(String::new());
    assert_eq!(
        config_with(target).validation_error(),
        Some(EMPTY_OVERRIDE_HOST.to_string())
    );
}

#[test]
fn validation_error_rejects_override_host_header_value() {
    let mut target = target_with("api-us.example.com");
    target.override_host = Some("api.example.com\r\nX-Injected: 1".to_string());
    assert_eq!(
        config_with(target).validation_error(),
        Some(OVERRIDE_HOST_HEADER.to_string())
    );
}

#[test]
fn validation_error_rejects_override_host_authority() {
    for host in [
        "not a host",
        "user@example.com",
        "example.com:abc",
        "example.com:0",
        "example.com:+80",
    ] {
        let mut target = target_with("api-us.example.com");
        target.override_host = Some(host.to_string());
        assert_eq!(
            config_with(target).validation_error(),
            Some(OVERRIDE_HOST_AUTHORITY.to_string()),
            "invalid Host authority must be rejected: {host}"
        );
    }
}

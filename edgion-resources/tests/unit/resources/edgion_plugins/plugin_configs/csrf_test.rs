use super::*;

#[test]
fn csrf_deserializes_secret_ref() {
    let json = r#"{"secretRef":{"name":"csrf-key"}}"#;
    let c: CsrfConfig = serde_json::from_str(json).unwrap();
    assert_eq!(c.secret_ref.as_ref().unwrap().name, "csrf-key");
    assert!(c.resolved_key.is_none());
    assert_eq!(c.expires, 7200);
    assert_eq!(c.name, "apisix-csrf-token");
}

#[test]
fn csrf_rejects_inline_key() {
    let json = r#"{"key":"leaked"}"#;
    assert!(serde_json::from_str::<CsrfConfig>(json).is_err());
}

#[test]
fn csrf_validation_requires_secret_ref_or_resolved() {
    use crate::resources::edgion_plugins::PluginValidation;
    let empty = CsrfConfig::default();
    assert!(empty.validation_error().is_some());
    let with_ref = CsrfConfig {
        secret_ref: Some(crate::resources::gateway::SecretObjectReference {
            group: None,
            kind: None,
            name: "k".into(),
            namespace: None,
        }),
        ..Default::default()
    };
    assert!(with_ref.validation_error().is_none());
    let with_resolved = CsrfConfig {
        resolved_key: Some("k".into()),
        ..Default::default()
    };
    assert!(with_resolved.validation_error().is_none());
}

#[test]
fn csrf_resolved_key_redacted_under_guard() {
    let cfg = CsrfConfig {
        resolved_key: Some("SUPER-SECRET".to_string()),
        ..Default::default()
    };
    let redacted = {
        let _g = crate::redact::AdminRedactGuard::new();
        serde_json::to_value(&cfg).unwrap()
    };
    assert_eq!(redacted["resolvedKey"], "[redacted]");
    let plain = serde_json::to_value(&cfg).unwrap();
    assert_eq!(plain["resolvedKey"], "SUPER-SECRET");
}

#[test]
fn csrf_validation_requires_a_positive_expiration() {
    for expires in [0, -1, i64::MIN] {
        let cfg = CsrfConfig {
            resolved_key: Some("k".into()),
            expires,
            ..Default::default()
        };
        assert_eq!(
            cfg.validation_error().as_deref(),
            Some("CSRF config: 'expires' must be positive")
        );
    }

    let cfg = CsrfConfig {
        resolved_key: Some("k".into()),
        expires: 1,
        ..Default::default()
    };
    assert!(cfg.validation_error().is_none());
}

#[test]
fn csrf_validation_bounds_the_shared_header_and_cookie_token_name() {
    for name in [
        default_name(),
        "x".repeat(HEADER_NAME_MAX_LEN as usize),
        "x-cookie".to_string(),
    ] {
        let cfg = CsrfConfig {
            resolved_key: Some("k".into()),
            name,
            ..Default::default()
        };
        assert!(cfg.validation_error().is_none());
    }

    for name in [
        String::new(),
        "bad name".to_string(),
        "bad:name".to_string(),
        "x".repeat(HEADER_NAME_MAX_LEN as usize + 1),
    ] {
        let cfg = CsrfConfig {
            resolved_key: Some("k".into()),
            name,
            ..Default::default()
        };
        assert_eq!(
            cfg.validation_error().as_deref(),
            Some("CSRF config: 'name' must be a valid HTTP token of at most 256 bytes")
        );
    }
}

#[test]
fn csrf_validation_rejects_the_cookie_header_name_case_insensitively() {
    for name in ["cookie", "Cookie", "COOKIE"] {
        let cfg = CsrfConfig {
            resolved_key: Some("k".into()),
            name: name.to_string(),
            ..Default::default()
        };
        assert_eq!(
            cfg.validation_error().as_deref(),
            Some("CSRF config: 'name' must not be 'cookie'")
        );
    }
}

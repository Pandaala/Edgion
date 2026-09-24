use super::*;

#[test]
fn test_default_values() {
    let config = HmacAuthConfig::default();
    assert_eq!(config.algorithms.len(), 3);
    assert_eq!(config.clock_skew, 300);
    assert_eq!(config.realm, "edgion");
    assert_eq!(config.secret_field, "secret");
    assert_eq!(config.username_field, "username");
    assert!(!config.validate_request_body);
}

#[test]
fn test_detect_validation_error_algorithms_empty() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            algorithms: vec![],
            ..Default::default()
        },
        ..Default::default()
    };
    let err = config.detect_validation_error();
    assert!(err.is_some());
    assert!(err.unwrap().contains("algorithms cannot be empty"));
}

#[test]
fn test_detect_validation_error_clock_skew_zero() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            clock_skew: 0,
            ..Default::default()
        },
        ..Default::default()
    };
    let err = config.detect_validation_error();
    assert!(err.is_some());
    assert!(err.unwrap().contains("clockSkew must be > 0"));
}

#[test]
fn test_validate_request_body_is_accepted() {
    // validateRequestBody is now a supported config: it is enforced at
    // runtime via the body gate, not rejected at config validation. With a
    // valid credential source it must produce no validation error.
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            validate_request_body: true,
            ..Default::default()
        },
        anonymous: Some("anon".to_string()),
        ..Default::default()
    };
    assert!(config.detect_validation_error().is_none());
}

#[test]
fn test_detect_validation_error_requires_credentials_or_anonymous() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig::default(),
        anonymous: None,
        ..Default::default()
    };
    let err = config.detect_validation_error();
    assert!(err.is_some());
    assert!(err.unwrap().contains("secretGroups"));
}

#[test]
fn test_detect_validation_error_allows_anonymous_without_credentials() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig::default(),
        anonymous: Some("guest".to_string()),
        ..Default::default()
    };
    let err = config.detect_validation_error();
    assert!(err.is_none());
}

#[test]
fn test_validate_secret_groups_empty() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            secret_groups: Some(vec![]),
            ..Default::default()
        },
        ..Default::default()
    };
    let err = config.detect_validation_error();
    assert_eq!(err.unwrap(), "secretGroups cannot be empty when present");
}

#[test]
fn test_validate_secret_groups_duplicate_name() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            secret_groups: Some(vec![
                HmacAuthSecretGroup {
                    name: "a".into(),
                    description: None,
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s1".into(),
                        namespace: None,
                    }],
                },
                HmacAuthSecretGroup {
                    name: "a".into(),
                    description: None,
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s2".into(),
                        namespace: None,
                    }],
                },
            ]),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(config
        .detect_validation_error()
        .unwrap()
        .contains("duplicate group name 'a'"));
}

#[test]
fn test_validate_secret_groups_invalid_name() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "-bad".into(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                }],
            }]),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(config
        .detect_validation_error()
        .unwrap()
        .contains("must start with alphanumeric"));
}

#[test]
fn test_validate_secret_groups_empty_refs() {
    let config = HmacAuthConfig {
        match_config: HmacMatchConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "a".into(),
                description: None,
                secret_refs: vec![],
            }]),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(config
        .detect_validation_error()
        .unwrap()
        .contains("group 'a' has no secretRefs"));
}

#[test]
fn standalone_wire_stays_flat_and_round_trips() {
    let config: HmacAuthConfig =
        serde_yaml::from_str("anonymous: guest\nrealm: edge\nvalidateRequestBody: true\nhideCredentials: true\n")
            .unwrap();
    assert_eq!(config.anonymous.as_deref(), Some("guest"));
    assert!(config.match_config.validate_request_body);
    let yaml = serde_yaml::to_string(&config).unwrap();
    assert!(yaml.contains("validateRequestBody: true"), "{yaml}");
    assert!(!yaml.contains("matchConfig"), "{yaml}");
}

#[test]
fn standalone_wire_keeps_lenient_unknown_field_behavior() {
    let config: HmacAuthConfig = serde_yaml::from_str("anonymous: guest\nfutureField: ignored\n").unwrap();
    assert_eq!(config.anonymous.as_deref(), Some("guest"));
}

#[test]
fn standalone_schema_preserves_defaults_and_flat_shape() {
    let schema = serde_json::to_value(schemars::schema_for!(HmacAuthConfig)).unwrap();
    let properties = schema
        .pointer("/properties")
        .and_then(serde_json::Value::as_object)
        .unwrap();
    assert!(properties.contains_key("validateRequestBody"));
    assert!(properties.contains_key("realm"));
    assert!(!properties.contains_key("matchConfig"));
    let required = schema
        .pointer("/required")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(!required.iter().any(|field| field == "realm"));
    assert!(!required.iter().any(|field| field == "validateRequestBody"));
}

#[test]
fn resolved_credentials_remain_redacted_through_flat_wire_adapter() {
    let mut config = HmacAuthConfig {
        anonymous: Some("guest".to_string()),
        ..Default::default()
    };
    config.match_config.resolved_credentials = Some(HashMap::from([(
        "alice".to_string(),
        HmacCredential {
            secret: b"do-not-expose".to_vec(),
            headers: HashMap::new(),
            group: "default".to_string(),
        },
    )]));
    let json = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_string(&config).unwrap()
    };
    assert!(!json.contains("do-not-expose"), "{json}");
    assert!(json.contains("[redacted]"), "{json}");
}

#[test]
fn resolved_credentials_round_trip_outside_admin_redaction() {
    let mut config = HmacAuthConfig::default();
    config.match_config.resolved_credentials = Some(HashMap::from([(
        "alice".to_string(),
        HmacCredential {
            secret: b"conf-sync-secret".to_vec(),
            headers: HashMap::new(),
            group: "default".to_string(),
        },
    )]));

    let wire = serde_json::to_vec(&config).unwrap();
    let decoded: HmacAuthConfig = serde_json::from_slice(&wire).unwrap();
    assert_eq!(
        decoded
            .match_config
            .resolved_credentials
            .as_ref()
            .and_then(|credentials| credentials.get("alice"))
            .map(|credential| credential.secret.as_slice()),
        Some(b"conf-sync-secret".as_slice())
    );
}

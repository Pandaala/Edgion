use super::*;

#[test]
fn test_default_config() {
    let config = KeyAuthConfig::default();
    assert_eq!(config.key_sources.len(), 2);
    assert_eq!(
        config.key_sources[0],
        KeyGet::Header {
            name: "apikey".to_string()
        }
    );
    assert_eq!(
        config.key_sources[1],
        KeyGet::Query {
            name: "apikey".to_string()
        }
    );
    assert_eq!(config.realm, "API Gateway");
    assert_eq!(config.key_field, "key");
    assert!(!config.hide_credentials);
    assert!(config.anonymous.is_none());
    assert!(config.secret_groups.is_none());
    assert!(config.upstream_header_fields.is_empty());
    assert!(config.resolved_keys.is_none());
}

#[test]
fn test_deserialize_minimal_config() {
    let json = r#"{}"#;
    let config: KeyAuthConfig = serde_json::from_str(json).unwrap();
    // Should use defaults
    assert_eq!(config.key_sources.len(), 2);
}

#[test]
fn test_deserialize_full_config() {
    let json = r#"{
            "keySources": [
                {"type": "header", "name": "X-API-Key"},
                {"type": "query", "name": "api_key"},
                {"type": "cookie", "name": "api_token"}
            ],
            "hideCredentials": true,
            "anonymous": "guest",
            "realm": "My API",
            "keyField": "apiKey",
            "upstreamHeaderFields": ["X-Consumer-Username", "X-Customer-ID"]
        }"#;
    let config: KeyAuthConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.key_sources.len(), 3);
    assert_eq!(
        config.key_sources[0],
        KeyGet::Header {
            name: "X-API-Key".to_string()
        }
    );
    assert_eq!(
        config.key_sources[1],
        KeyGet::Query {
            name: "api_key".to_string()
        }
    );
    assert_eq!(
        config.key_sources[2],
        KeyGet::Cookie {
            name: "api_token".to_string()
        }
    );
    assert!(config.hide_credentials);
    assert_eq!(config.anonymous, Some("guest".to_string()));
    assert_eq!(config.realm, "My API");
    assert_eq!(config.key_field, "apiKey");
    assert_eq!(config.upstream_header_fields.len(), 2);
}

#[test]
fn test_deserialize_yaml_config() {
    let yaml = r#"
keySources:
  - type: header
    name: X-API-Key
  - type: query
    name: api_key
hideCredentials: true
realm: "Production API"
"#;
    let config: KeyAuthConfig = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(config.key_sources.len(), 2);
    assert!(config.hide_credentials);
    assert_eq!(config.realm, "Production API");
}

#[test]
fn test_validate_default_config() {
    let config = KeyAuthConfig::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_validate_empty_key_sources() {
    let config = KeyAuthConfig {
        key_sources: vec![],
        ..Default::default()
    };
    assert_eq!(
        config.validate().unwrap_err(),
        "key_sources cannot be empty - at least one key source must be specified"
    );
}

#[test]
fn test_validate_empty_name_in_key_source() {
    let config = KeyAuthConfig {
        key_sources: vec![KeyGet::Header { name: "".to_string() }],
        ..Default::default()
    };
    assert_eq!(config.validate().unwrap_err(), "key_sources[0]: name cannot be empty");
}

#[test]
fn test_validate_unsupported_key_source() {
    let config = KeyAuthConfig {
        key_sources: vec![KeyGet::RemoteIp],
        ..Default::default()
    };
    let error = config.validate().unwrap_err();
    assert!(error.contains("unsupported source type"));
    assert!(error.contains("remoteIp"));
}

#[test]
fn test_validate_unsupported_request_path_key_source() {
    let config = KeyAuthConfig {
        key_sources: vec![KeyGet::RequestPath],
        ..Default::default()
    };
    let error = config.validate().unwrap_err();
    assert!(error.contains("unsupported source type"));
    assert!(error.contains("requestPath"));
}

#[test]
fn test_validate_unsupported_request_identity_key_sources() {
    for source in [KeyGet::DirectPeerIp, KeyGet::Host] {
        let expected_type = source.source_type();
        let config = KeyAuthConfig {
            key_sources: vec![source],
            ..Default::default()
        };
        let error = config.validate().unwrap_err();
        assert!(error.contains("unsupported source type"));
        assert!(error.contains(expected_type));
    }
}

#[test]
fn test_validate_empty_key_field() {
    let config = KeyAuthConfig {
        key_field: "".to_string(),
        ..Default::default()
    };
    assert_eq!(config.validate().unwrap_err(), "key_field cannot be empty");
}

#[test]
fn test_validate_empty_realm() {
    let config = KeyAuthConfig {
        realm: "".to_string(),
        ..Default::default()
    };
    assert_eq!(config.validate().unwrap_err(), "realm cannot be empty");
}

#[test]
fn test_validate_valid_custom_config() {
    let config = KeyAuthConfig {
        key_sources: vec![
            KeyGet::Header {
                name: "X-API-Key".to_string(),
            },
            KeyGet::Query {
                name: "api_key".to_string(),
            },
            KeyGet::Cookie {
                name: "token".to_string(),
            },
        ],
        key_field: "apiKey".to_string(),
        realm: "My API".to_string(),
        ..Default::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn test_validate_secret_groups_empty() {
    let config = KeyAuthConfig {
        secret_groups: Some(vec![]),
        ..Default::default()
    };
    assert_eq!(
        config.validate().unwrap_err(),
        "secret_groups cannot be empty when present"
    );
}

#[test]
fn test_validate_secret_groups_duplicate_name() {
    let config = KeyAuthConfig {
        secret_groups: Some(vec![
            KeyAuthSecretGroup {
                name: "a".into(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s1".into(),
                    namespace: None,
                }],
            },
            KeyAuthSecretGroup {
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
    };
    assert!(config.validate().unwrap_err().contains("duplicate group name 'a'"));
}

#[test]
fn test_validate_secret_groups_invalid_name() {
    let config = KeyAuthConfig {
        secret_groups: Some(vec![KeyAuthSecretGroup {
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
    };
    assert!(config.validate().unwrap_err().contains("must start with alphanumeric"));
}

#[test]
fn test_validate_secret_groups_empty_refs() {
    let config = KeyAuthConfig {
        secret_groups: Some(vec![KeyAuthSecretGroup {
            name: "a".into(),
            description: None,
            secret_refs: vec![],
        }]),
        ..Default::default()
    };
    assert!(config.validate().unwrap_err().contains("group 'a' has no secretRefs"));
}

#[test]
fn upstream_header_field_names_must_be_header_tokens() {
    for (name, valid) in [
        ("X-Customer-ID", true),
        ("X-Mixed!#$%&'*+-.^_`|~09", true),
        ("", false),
        ("bad name", false),
        ("bad:name", false),
        ("bad\rname", false),
        ("bad-é", false),
    ] {
        let config = KeyAuthConfig {
            upstream_header_fields: vec!["X-Valid".into(), name.into()],
            ..Default::default()
        };
        if valid {
            assert!(config.validate().is_ok(), "{name:?}");
        } else {
            assert_eq!(
                config.validate().unwrap_err(),
                "upstreamHeaderFields[1] must be a valid HTTP header name of at most 256 bytes"
            );
        }
    }
}

#[test]
fn anonymous_must_be_header_text_only_when_forwarded() {
    for (anonymous, valid) in [
        ("guest", true),
        ("café", true),
        ("a\u{1}b", false),
        ("a\u{7f}b", false),
        ("a\nb", false),
    ] {
        // Header names are case-insensitive, so any spelling forwards it.
        for field in ["X-Consumer-Username", "x-consumer-username"] {
            let forwarded = KeyAuthConfig {
                anonymous: Some(anonymous.into()),
                upstream_header_fields: vec![field.into()],
                ..Default::default()
            };
            assert!(forwarded.forwards_anonymous_username());
            if valid {
                assert!(forwarded.validate().is_ok());
            } else {
                assert_eq!(forwarded.validate().unwrap_err(), "anonymous is not valid header text");
            }
        }

        // Not written: the whitelist entry is absent.
        for fields in [vec![], vec!["X-Other".to_string()]] {
            let unforwarded = KeyAuthConfig {
                anonymous: Some(anonymous.into()),
                upstream_header_fields: fields,
                ..Default::default()
            };
            assert!(!unforwarded.forwards_anonymous_username());
            assert!(unforwarded.validate().is_ok());
        }
    }
}

#[test]
fn upstream_header_fields_reject_host_ignoring_case() {
    for name in [
        "Host",
        "host",
        "HOST",
        "hOsT",
        "Content-Length",
        "transfer-encoding",
        "Connection",
        "Upgrade",
        "TE",
    ] {
        let config = KeyAuthConfig {
            upstream_header_fields: vec!["X-Valid".into(), name.into()],
            ..Default::default()
        };
        assert_eq!(
            config.validate().unwrap_err(),
            "upstreamHeaderFields[1] cannot be Host or a framing/hop-by-hop header; use a dedicated identity header",
            "{name:?}"
        );
    }
    let config = KeyAuthConfig {
        upstream_header_fields: vec!["X-Host".into(), "Hosts".into()],
        ..Default::default()
    };
    assert!(config.validate().is_ok());
}

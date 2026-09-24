use super::*;
use crate::resources::edgion_plugins::PluginValidation;
use crate::resources::gateway::SecretObjectReference;

fn make_secret_ref() -> SecretObjectReference {
    SecretObjectReference {
        group: None,
        kind: None,
        name: "my-secret".to_string(),
        namespace: Some("default".to_string()),
    }
}

fn make_issuer_group(name: &str, issuer: &str) -> JwtIssuerGroup {
    JwtIssuerGroup {
        name: name.to_string(),
        description: None,
        issuers: vec![issuer.to_string()],
        secret_refs: vec![make_secret_ref()],
        algorithm: None,
        audiences: None,
    }
}

#[test]
fn test_no_credential_no_anonymous_returns_error() {
    let cfg = JwtAuthConfig::default();
    let err = cfg.validation_error();
    assert!(err.is_some(), "expected a validation error");
    let msg = err.unwrap();
    assert!(
        msg.contains("issuerGroups"),
        "error message should mention issuerGroups, got: {msg}"
    );
}

#[test]
fn test_resolved_groups_cannot_replace_operator_issuer_groups() {
    let cfg = JwtAuthConfig {
        resolved_groups_by_iss: Some(HashMap::from([(
            "https://injected.example".to_string(),
            ResolvedJwtIssuerGroup {
                group: "injected".to_string(),
                algorithm: JwtAlgorithm::HS256,
                audiences: None,
                credentials: HashMap::from([(
                    "kid".to_string(),
                    ResolvedJwtCredential {
                        key: Some("kid".to_string()),
                        secret: Some("injected-secret".to_string()),
                        public_key: None,
                    },
                )]),
            },
        )])),
        ..Default::default()
    };

    assert_eq!(
        cfg.validation_error(),
        Some("issuerGroups is required when anonymous is not set".to_string())
    );
}

#[test]
fn test_anonymous_set_no_credential_is_valid() {
    let cfg = JwtAuthConfig {
        anonymous: Some("guest".to_string()),
        ..Default::default()
    };
    assert_eq!(cfg.validation_error(), None);
}

#[test]
fn test_empty_header_returns_error() {
    let cfg = JwtAuthConfig {
        issuer_groups: Some(vec![make_issuer_group("g", "https://a.example")]),
        header: "   ".to_string(),
        ..Default::default()
    };
    let err = cfg.validation_error();
    assert!(err.is_some());
    assert!(err.unwrap().contains("header"));
}

#[test]
fn test_valid_config_with_issuer_group_returns_none() {
    let cfg = JwtAuthConfig {
        issuer_groups: Some(vec![make_issuer_group("g", "https://a.example")]),
        ..Default::default()
    };
    assert_eq!(cfg.validation_error(), None);
}

#[test]
fn test_validate_issuer_groups_empty() {
    let config = JwtAuthConfig {
        issuer_groups: Some(vec![]),
        anonymous: None,
        ..Default::default()
    };
    let err = config.validation_error();
    assert_eq!(err.unwrap(), "issuerGroups cannot be empty when present");
}

#[test]
fn test_validate_issuer_groups_duplicate_name() {
    let config = JwtAuthConfig {
        issuer_groups: Some(vec![
            JwtIssuerGroup {
                name: "a".into(),
                description: None,
                issuers: vec!["https://a.example".into()],
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s1".into(),
                    namespace: None,
                }],
                algorithm: None,
                audiences: None,
            },
            JwtIssuerGroup {
                name: "a".into(),
                description: None,
                issuers: vec!["https://b.example".into()],
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s2".into(),
                    namespace: None,
                }],
                algorithm: None,
                audiences: None,
            },
        ]),
        anonymous: None,
        ..Default::default()
    };
    assert!(config.validation_error().unwrap().contains("duplicate group name 'a'"));
}

#[test]
fn test_validate_issuer_groups_invalid_name() {
    let config = JwtAuthConfig {
        issuer_groups: Some(vec![JwtIssuerGroup {
            name: "-bad".into(),
            description: None,
            issuers: vec!["https://a.example".into()],
            secret_refs: vec![SecretObjectReference {
                group: None,
                kind: None,
                name: "s".into(),
                namespace: None,
            }],
            algorithm: None,
            audiences: None,
        }]),
        anonymous: None,
        ..Default::default()
    };
    assert!(config
        .validation_error()
        .unwrap()
        .contains("must start with alphanumeric"));
}

#[test]
fn test_validate_issuer_groups_empty_issuers() {
    let config = JwtAuthConfig {
        issuer_groups: Some(vec![JwtIssuerGroup {
            name: "a".into(),
            description: None,
            issuers: vec![],
            secret_refs: vec![SecretObjectReference {
                group: None,
                kind: None,
                name: "s".into(),
                namespace: None,
            }],
            algorithm: None,
            audiences: None,
        }]),
        anonymous: None,
        ..Default::default()
    };
    assert!(config.validation_error().unwrap().contains("group 'a' has no issuers"));
}

#[test]
fn test_validate_cross_group_iss_collision() {
    let dup_iss = "https://shared.example";
    let config = JwtAuthConfig {
        issuer_groups: Some(vec![
            JwtIssuerGroup {
                name: "a".into(),
                description: None,
                issuers: vec![dup_iss.into()],
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s1".into(),
                    namespace: None,
                }],
                algorithm: None,
                audiences: None,
            },
            JwtIssuerGroup {
                name: "b".into(),
                description: None,
                issuers: vec![dup_iss.into()],
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s2".into(),
                    namespace: None,
                }],
                algorithm: None,
                audiences: None,
            },
        ]),
        anonymous: None,
        ..Default::default()
    };
    let err = config.validation_error().unwrap();
    assert!(err.contains("iss must be unique across groups"), "got: {}", err);
    assert!(err.contains(dup_iss), "got: {}", err);
}

#[test]
fn validation_rejects_case_insensitive_claim_header_destination_collision() {
    let config = JwtAuthConfig {
        anonymous: Some("anonymous".to_string()),
        claims_to_headers: Some(HashMap::from([
            ("sub".to_string(), "X-Identity".to_string()),
            ("email".to_string(), "x-identity".to_string()),
        ])),
        ..Default::default()
    };

    assert_eq!(
        config.validation_error().as_deref(),
        Some(super::super::DUPLICATE_HEADER_DESTINATION)
    );
}

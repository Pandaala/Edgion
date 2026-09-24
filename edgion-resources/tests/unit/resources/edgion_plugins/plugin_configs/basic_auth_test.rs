use super::BasicAuthConfig;
use std::collections::HashMap;

#[test]
fn resolved_password_debug_redaction_preserves_transport() {
    for password in ["private-password", "{SHA}private-hash"] {
        let config = BasicAuthConfig {
            resolved_users: Some(HashMap::from([(
                "alice".into(),
                super::ResolvedBasicAuthUser {
                    password: password.into(),
                    group: "private-group".into(),
                },
            )])),
            ..Default::default()
        };
        for debug in [format!("{config:?}"), format!("{config:#?}")] {
            assert!(debug.contains("[redacted]"));
            assert!(!debug.contains(password));
            assert!(!debug.contains("private-group"));
        }
        let wire = serde_json::to_string(&config).unwrap();
        let decoded: BasicAuthConfig = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded.resolved_users.unwrap()["alice"].password, password);
        let _guard = crate::redact::AdminRedactGuard::new();
        let admin = serde_json::to_value(&config).unwrap();
        assert_eq!(admin["resolvedUsers"], "[redacted]");
    }
}

#[test]
fn test_detect_validation_error_requires_creds_or_anonymous() {
    let cfg = BasicAuthConfig::default();
    assert!(cfg
        .detect_validation_error()
        .is_some_and(|e| e.contains("secretGroups")));
}

#[test]
fn test_default_auth_failure_delay_ms_is_zero() {
    let cfg = BasicAuthConfig::default();
    assert_eq!(cfg.auth_failure_delay_ms, 0);
}

#[test]
fn test_detect_validation_error_allows_resolved_users() {
    use super::ResolvedBasicAuthUser;
    let cfg = BasicAuthConfig {
        resolved_users: Some(HashMap::from([(
            "alice".to_string(),
            ResolvedBasicAuthUser {
                password: "pass".to_string(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    };
    assert!(cfg.detect_validation_error().is_none());
}

use crate::resources::edgion_plugins::plugin_configs::BasicAuthSecretGroup;
use crate::resources::gateway::SecretObjectReference;

fn make_sor(name: &str) -> SecretObjectReference {
    SecretObjectReference {
        group: None,
        kind: None,
        name: name.to_string(),
        namespace: None,
    }
}

#[test]
fn test_validate_secret_groups_empty() {
    let cfg = BasicAuthConfig {
        secret_groups: Some(vec![]),
        anonymous: None,
        ..Default::default()
    };
    assert_eq!(
        cfg.detect_validation_error().unwrap(),
        "secretGroups cannot be empty when present"
    );
}

#[test]
fn test_validate_secret_groups_duplicate_name() {
    let cfg = BasicAuthConfig {
        secret_groups: Some(vec![
            BasicAuthSecretGroup {
                name: "a".into(),
                description: None,
                secret_refs: vec![make_sor("s1")],
            },
            BasicAuthSecretGroup {
                name: "a".into(),
                description: None,
                secret_refs: vec![make_sor("s2")],
            },
        ]),
        anonymous: None,
        ..Default::default()
    };
    assert!(cfg
        .detect_validation_error()
        .unwrap()
        .contains("duplicate group name 'a'"));
}

#[test]
fn test_validate_secret_groups_invalid_name() {
    let cfg = BasicAuthConfig {
        secret_groups: Some(vec![BasicAuthSecretGroup {
            name: "-bad".into(),
            description: None,
            secret_refs: vec![make_sor("s")],
        }]),
        anonymous: None,
        ..Default::default()
    };
    assert!(cfg
        .detect_validation_error()
        .unwrap()
        .contains("must start with alphanumeric"));
}

#[test]
fn test_validate_secret_groups_empty_refs() {
    let cfg = BasicAuthConfig {
        secret_groups: Some(vec![BasicAuthSecretGroup {
            name: "a".into(),
            description: None,
            secret_refs: vec![],
        }]),
        anonymous: None,
        ..Default::default()
    };
    assert!(cfg
        .detect_validation_error()
        .unwrap()
        .contains("group 'a' has no secretRefs"));
}

#[test]
fn test_dual_group_yaml_deserializes() {
    let yaml = r#"
secretGroups:
  - name: partner-a
    secretRefs:
      - name: secret-a
        namespace: edgion-default
  - name: partner-b
    secretRefs:
      - name: secret-b
        namespace: edgion-default
realm: "Dynamic Partner API"
"#;
    let cfg: BasicAuthConfig = serde_yaml::from_str(yaml).unwrap();
    let groups = cfg.secret_groups.unwrap();
    assert_eq!(groups.len(), 2, "should parse 2 groups, got {}", groups.len());
    assert_eq!(groups[0].name, "partner-a");
    assert_eq!(groups[1].name, "partner-b");
}

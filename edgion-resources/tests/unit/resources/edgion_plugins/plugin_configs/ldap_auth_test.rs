use super::*;
use serde_json::json;

#[test]
fn test_default_config() {
    let cfg = LdapAuthConfig::default();
    assert_eq!(cfg.ldap_port, 389);
    assert!(!cfg.ldaps);
    assert!(!cfg.start_tls);
    assert!(cfg.tls.verify.is_none());
    assert!(cfg.tls.validation.is_none());
    assert!(cfg.tls.client_certificate_ref.is_none());
    assert!(cfg.resolved_ca_certificates.is_none());
    assert!(cfg.resolved_client_certificate.is_none());
    assert_eq!(cfg.header_type, "ldap");
    assert_eq!(cfg.realm, "API Gateway");
    assert_eq!(cfg.timeout, 10_000);
    assert_eq!(cfg.credential_identifier_header, "X-Credential-Identifier");
    assert_eq!(cfg.anonymous_header, "X-Anonymous-Consumer");
}

#[test]
fn test_validation_required_fields() {
    let cfg = LdapAuthConfig::default();
    assert_eq!(cfg.get_validation_error(), Some("ldapHost is required"));
}

#[test]
fn test_validation_ldaps_starttls_conflict() {
    let cfg = LdapAuthConfig {
        ldap_host: "ldap.example.com".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        attribute: "uid".to_string(),
        ldaps: true,
        start_tls: true,
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("ldaps and startTls cannot both be true")
    );
}

#[test]
fn test_validation_bind_dn_template() {
    let cfg = LdapAuthConfig {
        ldap_host: "ldap.example.com".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        attribute: "uid".to_string(),
        bind_dn_template: Some("uid=user,dc=example,dc=com".to_string()),
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("bindDnTemplate must contain {username}")
    );
}

#[test]
fn ldap_cache_fields_are_not_part_of_the_schema() {
    let schema = schemars::schema_for!(LdapAuthConfig);
    let value = json!({
        "ldapHost": "ldap.example.com",
        "baseDn": "dc=example,dc=com",
        "attribute": "uid",
        "cacheTtl": 60,
        "lruCache": {"capacity": 512, "shards": 4},
        "memoryCache": {"maxEntries": 512}
    });
    assert_eq!(
        crate::schema_check::unknown_key_paths(&value, &schema),
        vec![
            "cacheTtl".to_string(),
            "lruCache".to_string(),
            "memoryCache".to_string(),
        ]
    );
}

#[test]
fn test_validation_ok() {
    let cfg = LdapAuthConfig {
        ldap_host: "ldap.example.com".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        attribute: "uid".to_string(),
        bind_dn_template: Some("uid={username},dc=example,dc=com".to_string()),
        ..Default::default()
    };
    assert_eq!(cfg.get_validation_error(), None);
}

#[test]
fn ldap_outbound_defaults_all_none() {
    let c: LdapAuthConfig = serde_json::from_value(serde_json::json!({
        "ldapHost": "ldap.example.com",
        "baseDn": "dc=example,dc=com",
        "attribute": "uid"
    }))
    .unwrap();
    assert!(c.tls.verify.is_none());
    assert!(c.tls.validation.is_none());
    assert!(c.tls.client_certificate_ref.is_none());
}

#[test]
fn ldap_outbound_accepts_verify_and_validation() {
    let c: LdapAuthConfig = serde_json::from_value(serde_json::json!({
        "ldapHost": "ldap.example.com",
        "baseDn": "dc=example,dc=com",
        "attribute": "uid",
        "tls": { "verify": false }
    }))
    .unwrap();
    assert_eq!(c.tls.verify, Some(false));
    assert!(c.tls.validation.is_none());
}

#[test]
fn resolved_fields_absent_from_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(LdapAuthConfig)).unwrap();
    let props = &schema["properties"];
    assert!(
        props.get("resolvedCaCertificates").is_none() && props.get("resolved_ca_certificates").is_none(),
        "resolvedCaCertificates must not appear in schema"
    );
    assert!(
        props.get("resolvedClientCertificate").is_none() && props.get("resolved_client_certificate").is_none(),
        "resolvedClientCertificate must not appear in schema"
    );
}

fn valid_base() -> LdapAuthConfig {
    LdapAuthConfig {
        ldap_host: "ldap.example.com".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        attribute: "uid".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_validation_identity_header_config() {
    assert_eq!(valid_base().get_validation_error(), None);
    let cases = [
        (
            LdapAuthConfig {
                credential_identifier_header: "X Credential".to_string(),
                ..valid_base()
            },
            "credentialIdentifierHeader is not a valid header name",
        ),
        (
            LdapAuthConfig {
                anonymous_header: String::new(),
                ..valid_base()
            },
            "anonymousHeader is not a valid header name",
        ),
        (
            LdapAuthConfig {
                anonymous: Some("guest\nadmin".to_string()),
                ..valid_base()
            },
            "anonymous is not valid header text",
        ),
    ];
    for (config, expected) in cases {
        assert_eq!(config.get_validation_error(), Some(expected));
    }
    let anonymous_ok = LdapAuthConfig {
        anonymous: Some("guest user".to_string()),
        ..valid_base()
    };
    assert_eq!(anonymous_ok.get_validation_error(), None);
}

fn search_base_config() -> LdapAuthConfig {
    LdapAuthConfig {
        ldaps: true,
        bind_dn: Some("cn=svc,dc=example,dc=com".to_string()),
        bind_password_ref: Some(crate::resources::gateway::SecretObjectReference {
            group: None,
            kind: None,
            name: "ldap-svc".to_string(),
            namespace: None,
        }),
        ..valid_base()
    }
}

#[test]
fn test_validation_search_mode() {
    assert_eq!(search_base_config().get_validation_error(), None);
    let start_tls = LdapAuthConfig {
        ldaps: false,
        start_tls: true,
        ..search_base_config()
    };
    assert_eq!(start_tls.get_validation_error(), None);

    let cases: Vec<(LdapAuthConfig, &str)> = vec![
        (
            LdapAuthConfig {
                bind_dn: Some("  ".to_string()),
                ..search_base_config()
            },
            "bindDn cannot be empty",
        ),
        (
            LdapAuthConfig {
                bind_dn_template: Some("uid={username},dc=x".to_string()),
                ..search_base_config()
            },
            "bindDn and bindDnTemplate cannot both be set",
        ),
        (
            LdapAuthConfig {
                bind_password_ref: None,
                resolved_bind_password: Some("forged".to_string()),
                ..search_base_config()
            },
            "bindPasswordRef is required with bindDn",
        ),
        (
            LdapAuthConfig {
                ldaps: false,
                ..search_base_config()
            },
            "bindDn requires ldaps or startTls",
        ),
        (
            LdapAuthConfig {
                search_base: Some(" ".to_string()),
                ..search_base_config()
            },
            "searchBase cannot be empty",
        ),
        (
            LdapAuthConfig {
                search_base: Some("example.org".to_string()),
                ..search_base_config()
            },
            "searchBase (or baseDn when unset) is not a valid DN",
        ),
        (
            LdapAuthConfig {
                search_base: None,
                base_dn: "example.org".to_string(),
                ..search_base_config()
            },
            "searchBase (or baseDn when unset) is not a valid DN",
        ),
        (
            LdapAuthConfig {
                bind_dn: Some("svc@example.com".to_string()),
                ..search_base_config()
            },
            "bindDn must be a DN in string form (attribute names, no #hex values)",
        ),
        (
            LdapAuthConfig {
                bind_dn: Some("2.5.4.3=svc,dc=example,dc=com".to_string()),
                ..search_base_config()
            },
            "bindDn must be a DN in string form (attribute names, no #hex values)",
        ),
        (
            LdapAuthConfig {
                bind_dn: Some("cn=#0403737663,dc=example,dc=com".to_string()),
                ..search_base_config()
            },
            "bindDn must be a DN in string form (attribute names, no #hex values)",
        ),
        (
            LdapAuthConfig {
                attribute: "user name".to_string(),
                ..search_base_config()
            },
            "attribute is not a valid attribute name",
        ),
        (
            LdapAuthConfig {
                tls: OutboundTlsLocal {
                    verify: Some(false),
                    ..Default::default()
                },
                ..search_base_config()
            },
            "bindDn requires TLS certificate verification",
        ),
        (
            LdapAuthConfig {
                search_filter: Some("(objectClass=person)".to_string()),
                ..search_base_config()
            },
            "searchFilter must contain {username}",
        ),
        (
            LdapAuthConfig {
                search_filter: Some("(&(uid={username})".to_string()),
                ..search_base_config()
            },
            "searchFilter is invalid",
        ),
        (
            LdapAuthConfig {
                bind_dn: None,
                search_filter: Some("(uid={username})".to_string()),
                ..search_base_config()
            },
            "searchFilter, searchBase and bindPasswordRef require bindDn",
        ),
    ];
    for (config, expected) in cases {
        assert_eq!(config.get_validation_error(), Some(expected), "{expected}");
    }
}

#[test]
fn test_bind_only_mode_ignores_search_rules() {
    // An attribute the filter grammar rejects stays valid in bind-only mode.
    let config = LdapAuthConfig {
        attribute: "user name".to_string(),
        ..valid_base()
    };
    assert_eq!(config.get_validation_error(), None);
    assert!(config.bind_dn.is_none());
}

#[test]
fn test_search_mode_defaults_and_serialization() {
    let config = search_base_config();
    assert_eq!(config.effective_search_filter(), "(uid={username})");
    let parsed: LdapAuthConfig = serde_json::from_value(json!({
        "ldapHost": "ldap.example.com",
        "baseDn": "dc=example,dc=com",
        "attribute": "sAMAccountName",
        "ldaps": true,
        "bindDn": "CN=svc,DC=example,DC=com",
        "bindPasswordRef": {"name": "ldap-svc"},
        "searchFilter": "(&(objectClass=user)(sAMAccountName={username}))",
        "searchBase": "DC=example,DC=com"
    }))
    .unwrap();
    assert_eq!(parsed.get_validation_error(), None);
    assert_eq!(parsed.search_base.as_deref(), Some("DC=example,DC=com"));
    let with_secret = LdapAuthConfig {
        resolved_bind_password: Some("top-secret".to_string()),
        ..parsed
    };
    let serialized = serde_json::to_string(&with_secret).unwrap();
    let debug = format!("{with_secret:?}");
    assert!(!debug.contains("top-secret"));
    // conf_sync serialization carries the value; admin output redacts it (redact_test.rs).
    assert!(
        serialized.contains("\"resolvedBindPassword\":\"top-secret\""),
        "{serialized}"
    );
}

#[test]
fn test_search_mode_verify_false_with_san_policy_is_accepted() {
    let config = LdapAuthConfig {
        tls: OutboundTlsLocal {
            verify: Some(false),
            validation: Some(
                serde_json::from_value(json!({
                    "subjectAltNames": [{"type": "Hostname", "hostname": "ldap.example.com"}]
                }))
                .unwrap(),
            ),
            ..Default::default()
        },
        ..search_base_config()
    };
    assert_eq!(config.get_validation_error(), None);
}

#[test]
fn test_header_checks_precede_search_mode_checks() {
    let config = LdapAuthConfig {
        credential_identifier_header: "bad header".to_string(),
        ldaps: false,
        ..search_base_config()
    };
    assert_eq!(
        config.get_validation_error(),
        Some("credentialIdentifierHeader is not a valid header name")
    );
}

fn group_config(any_of: Option<Vec<&str>>, all_of: Option<Vec<&str>>) -> LdapAuthConfig {
    let owned = |groups: Option<Vec<&str>>| groups.map(|groups| groups.into_iter().map(str::to_string).collect());
    LdapAuthConfig {
        required_groups_any_of: owned(any_of),
        required_groups_all_of: owned(all_of),
        ..search_base_config()
    }
}

#[test]
fn test_group_requirements_validation() {
    const GROUP: &str = "cn=gw-users,ou=groups,dc=example,dc=com";
    assert_eq!(group_config(Some(vec![GROUP]), None).get_validation_error(), None);
    assert_eq!(
        group_config(None, Some(vec![GROUP, GROUP])).get_validation_error(),
        None
    );
    assert_eq!(
        group_config(Some(vec![" cn=a,dc=x "]), Some(vec![GROUP])).get_validation_error(),
        None
    );
    let thirty_two = vec![GROUP; MAX_REQUIRED_GROUPS];
    assert_eq!(
        group_config(Some(thirty_two.clone()), None).get_validation_error(),
        None
    );
    let max_dn = format!("cn={},dc=x", "a".repeat(MAX_GROUP_DN_LEN - 8));
    assert_eq!(max_dn.len(), MAX_GROUP_DN_LEN);
    assert_eq!(
        group_config(Some(vec![max_dn.as_str()]), None).get_validation_error(),
        None
    );

    let count = "requiredGroupsAnyOf and requiredGroupsAllOf need 1 to 32 groups each";
    let entries = "requiredGroupsAnyOf and requiredGroupsAllOf entries must be valid DNs";
    let mut thirty_three = thirty_two;
    thirty_three.push(GROUP);
    // The cap applies after trimming.
    let padded = format!("  {max_dn}  ");
    assert_eq!(
        group_config(None, Some(vec![padded.as_str()])).get_validation_error(),
        None
    );
    let too_long = format!("{max_dn}a");
    let cases: Vec<(LdapAuthConfig, &str)> = vec![
        (group_config(Some(vec![]), None), count),
        (group_config(None, Some(vec![])), count),
        (group_config(Some(thirty_three.clone()), None), count),
        (group_config(None, Some(thirty_three)), count),
        (group_config(Some(vec!["  "]), None), entries),
        (group_config(None, Some(vec!["gw-users"])), entries),
        (group_config(Some(vec![too_long.as_str()]), None), entries),
        (
            LdapAuthConfig {
                bind_dn: None,
                bind_password_ref: None,
                resolved_bind_password: None,
                search_base: None,
                search_filter: None,
                ..group_config(Some(vec![GROUP]), None)
            },
            "requiredGroupsAnyOf and requiredGroupsAllOf require bindDn",
        ),
        (
            LdapAuthConfig {
                group_attribute: "member of".to_string(),
                ..group_config(Some(vec![GROUP]), None)
            },
            "groupAttribute is not a valid attribute name",
        ),
        (
            LdapAuthConfig {
                group_attribute: "a".repeat(MAX_GROUP_ATTRIBUTE_LEN + 1),
                ..group_config(Some(vec![GROUP]), None)
            },
            "groupAttribute is not a valid attribute name",
        ),
    ];
    for (config, expected) in cases {
        assert_eq!(config.get_validation_error(), Some(expected), "{expected}");
    }

    // Attribute at the cap, trimmed.
    let config = LdapAuthConfig {
        group_attribute: format!(" {} ", "a".repeat(MAX_GROUP_ATTRIBUTE_LEN)),
        ..group_config(Some(vec![GROUP]), None)
    };
    assert_eq!(config.get_validation_error(), None);
    // Without a group list the attribute and nesting flag are inert.
    let inert = LdapAuthConfig {
        group_attribute: "member of".to_string(),
        nested_groups: true,
        ..search_base_config()
    };
    assert_eq!(inert.get_validation_error(), None);
    assert!(!inert.has_group_requirements());
}

#[test]
fn test_group_requirements_serde() {
    let defaults: LdapAuthConfig = serde_json::from_value(json!({
        "ldapHost": "ldap.example.com",
        "baseDn": "dc=example,dc=com",
        "attribute": "uid"
    }))
    .unwrap();
    assert_eq!(defaults.group_attribute, "memberOf");
    assert!(!defaults.nested_groups);
    assert!(defaults.required_groups_any_of.is_none() && defaults.required_groups_all_of.is_none());

    let parsed: LdapAuthConfig = serde_json::from_value(json!({
        "ldapHost": "ldap.example.com",
        "baseDn": "dc=example,dc=com",
        "attribute": "uid",
        "ldaps": true,
        "bindDn": "cn=svc,dc=example,dc=com",
        "bindPasswordRef": {"name": "ldap-svc"},
        "requiredGroupsAnyOf": ["cn=a,dc=example,dc=com"],
        "requiredGroupsAllOf": [],
        "groupAttribute": "isMemberOf",
        "nestedGroups": true
    }))
    .unwrap();
    assert_eq!(
        parsed.required_groups_any_of.as_deref(),
        Some(&["cn=a,dc=example,dc=com".to_string()][..])
    );
    // An explicit empty list stays distinguishable from an absent one and is rejected.
    assert_eq!(parsed.required_groups_all_of.as_deref(), Some(&[][..]));
    assert_eq!(
        parsed.get_validation_error(),
        Some("requiredGroupsAnyOf and requiredGroupsAllOf need 1 to 32 groups each")
    );
    assert_eq!(parsed.group_attribute, "isMemberOf");
    assert!(parsed.nested_groups);
    let value = serde_json::to_value(&parsed).unwrap();
    assert_eq!(value["requiredGroupsAllOf"], json!([]));
    assert_eq!(value["groupAttribute"], json!("isMemberOf"));
}

#[test]
fn test_group_list_null_decodes_and_is_rejected_by_validation() {
    for field in ["requiredGroupsAnyOf", "requiredGroupsAllOf"] {
        let mut value = json!({
            "ldapHost": "ldap.example.com",
            "baseDn": "dc=example,dc=com",
            "attribute": "uid",
            "ldaps": true,
            "bindDn": "cn=svc,dc=example,dc=com",
            "bindPasswordRef": {"name": "ldap-svc"}
        });
        value[field] = serde_json::Value::Null;
        // Decoding succeeds (a decode error would drop the whole resource);
        // the gate is not removed: validation rejects the empty list.
        let config: LdapAuthConfig = serde_json::from_value(value).unwrap();
        assert!(config.has_group_requirements(), "{field}");
        assert_eq!(
            config.get_validation_error(),
            Some("requiredGroupsAnyOf and requiredGroupsAllOf need 1 to 32 groups each"),
            "{field}"
        );
    }
}

#[test]
fn test_group_null_items_and_null_attribute_decode() {
    let config: LdapAuthConfig = serde_json::from_value(json!({
        "ldapHost": "ldap.example.com",
        "baseDn": "dc=example,dc=com",
        "attribute": "uid",
        "ldaps": true,
        "bindDn": "cn=svc,dc=example,dc=com",
        "bindPasswordRef": {"name": "ldap-svc"},
        "requiredGroupsAnyOf": [null],
        "groupAttribute": null
    }))
    .unwrap();
    // A null item is an empty entry (rejected); a null attribute is the default.
    assert_eq!(config.required_groups_any_of.as_deref(), Some(&[String::new()][..]));
    assert_eq!(config.group_attribute, "memberOf");
    assert_eq!(
        config.get_validation_error(),
        Some("requiredGroupsAnyOf and requiredGroupsAllOf entries must be valid DNs")
    );
}

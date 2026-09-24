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

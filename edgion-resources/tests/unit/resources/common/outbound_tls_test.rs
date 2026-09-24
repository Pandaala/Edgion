use super::*;

#[test]
fn validation_shape_parses_and_camelcases() {
    let json = r#"{"validation":{"caCertificateRefs":[{"kind":"Secret","name":"idp-ca"},{"kind":"ConfigMap","name":"corp-ca"}],"hostname":"idp.example.com","subjectAltNames":[{"type":"Hostname","hostname":"idp.example.com"}]},"clientCertificateRef":{"name":"idp-client"}}"#;
    let l: OutboundTlsLocal = serde_json::from_str(json).unwrap();
    let v = l.validation.unwrap();
    assert_eq!(v.ca_certificate_refs.unwrap().len(), 2);
    assert_eq!(v.hostname.as_deref(), Some("idp.example.com"));
    assert_eq!(v.subject_alt_names.unwrap().len(), 1);
    assert_eq!(l.client_certificate_ref.unwrap().name, "idp-client");
}

#[test]
fn well_known_and_refs_both_optional_both_absent_ok() {
    let l: OutboundTlsLocal = serde_json::from_str(r#"{"verify":true}"#).unwrap();
    assert!(l.validation.is_none());
}

#[test]
fn legacy_ca_source_field_no_longer_a_variant() {
    // CaSource is deleted; this field must be rejected at parse time by
    // deny_unknown_fields to prevent silent no-op when operators forget to migrate.
    // Before deny_unknown_fields, this would be silently ignored; now it fails loudly.
    let r: Result<OutboundTlsLocal, _> = serde_json::from_str(r#"{"caSource":"SecretRef"}"#);
    assert!(r.is_err(), "legacy caSource field should be rejected, got: {:?}", r);
}

#[test]
fn global_verify_defaults_true_when_absent() {
    let g: OutboundTlsConfig = serde_json::from_str("{}").unwrap();
    assert!(g.verify);
}

/// `resolved_ca_certificates` is a controller-resolved field: it must stay out of
/// the JSON schema (`#[schemars(skip)]`) so the structural CRD coverage guard
/// keeps in sync.
#[test]
fn resolved_ca_certificates_absent_from_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(OutboundTlsConfig)).unwrap();
    let props = &schema["properties"];
    assert!(
        props.get("resolvedCaCertificates").is_none() && props.get("resolved_ca_certificates").is_none(),
        "resolved_ca_certificates must not appear in the schema, got: {props}"
    );
}

/// In an admin-redact scope the resolved Secrets serialize as `[redacted]`,
/// while conf_sync (no guard) still ships the real values.
#[test]
fn resolved_ca_certificates_redacted_in_admin_output() {
    let cfg = OutboundTlsConfig {
        resolved_ca_certificates: Some(vec![k8s_openapi::api::core::v1::Secret {
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        }]),
        ..Default::default()
    };

    let admin = {
        let _g = crate::redact::AdminRedactGuard::new();
        serde_json::to_value(&cfg).unwrap()
    };
    assert_eq!(admin["resolvedCaCertificates"], "[redacted]");

    let sync = serde_json::to_value(&cfg).unwrap();
    assert!(
        sync["resolvedCaCertificates"].is_array(),
        "conf_sync must ship the real Secret array, got: {}",
        sync["resolvedCaCertificates"]
    );
}

#[test]
fn resolved_ca_certificates_is_vec_and_absent_from_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(OutboundTlsConfig)).unwrap();
    let props = &schema["properties"];
    assert!(props.get("resolvedCaCertificates").is_none());
    assert!(props.get("validation").is_some());
}

/// `resolved_client_certificate` is a controller-resolved field: it must stay out of
/// the JSON schema (`#[schemars(skip)]`) so the structural CRD coverage guard
/// keeps in sync (symmetric with `resolved_ca_certificates`).
#[test]
fn resolved_client_certificate_absent_from_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(OutboundTlsConfig)).unwrap();
    let props = &schema["properties"];
    assert!(
        props.get("resolvedClientCertificate").is_none() && props.get("resolved_client_certificate").is_none(),
        "resolved_client_certificate must not appear in the schema, got: {props}"
    );
}

#[test]
fn outbound_tls_local_rejects_legacy_keys() {
    // Old schema fields (caSource/caCert/caSecretRef/clientSecretRef) MUST
    // be rejected at parse time to prevent silent no-op when operators
    // forget to migrate. They are gone from the schema; serde must error
    // rather than ignore.
    for legacy in [
        r#"{"caSource":"SecretRef"}"#,
        r#"{"caCert":"---pem---"}"#,
        r#"{"caSecretRef":{"name":"ca"}}"#,
        r#"{"clientSecretRef":{"name":"id"}}"#,
    ] {
        let r: Result<OutboundTlsLocal, _> = serde_json::from_str(legacy);
        assert!(
            r.is_err(),
            "legacy key in {} should be rejected by deny_unknown_fields, got: {:?}",
            legacy,
            r
        );
    }
}

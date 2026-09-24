use super::*;

#[test]
fn tls_config_minimal_yaml_parses() {
    let yaml = "enabled: true";
    let cfg: TlsConfig = serde_yaml::from_str(yaml).expect("should parse");
    assert!(cfg.enabled);
    assert!(cfg.outbound.verify.is_none());
}

#[test]
fn tls_config_accepts_validation_and_client_ref() {
    let c: TlsConfig = serde_json::from_str(
            r#"{"enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},"clientCertificateRef":{"name":"id"}}"#,
        ).unwrap();
    assert_eq!(c.outbound.verify, Some(false));
    assert!(c.outbound.validation.unwrap().ca_certificate_refs.is_some());
    assert_eq!(c.outbound.client_certificate_ref.unwrap().name, "id");
}

#[test]
fn tls_config_rejects_legacy_outbound_keys() {
    // Flatten + deny may interact unpredictably; verify the LinkSys TlsConfig
    // path also rejects legacy outbound keys (otherwise it's a silent no-op
    // for redis/etcd/es operators).
    for legacy in [
        r#"{"enabled":true,"caSource":"SecretRef"}"#,
        r#"{"enabled":true,"caCert":"---pem---"}"#,
        r#"{"enabled":true,"caSecretRef":{"name":"ca"}}"#,
        r#"{"enabled":true,"clientSecretRef":{"name":"id"}}"#,
    ] {
        let r: Result<TlsConfig, _> = serde_json::from_str(legacy);
        assert!(r.is_err(), "legacy key in {} should be rejected, got: {:?}", legacy, r);
    }
}

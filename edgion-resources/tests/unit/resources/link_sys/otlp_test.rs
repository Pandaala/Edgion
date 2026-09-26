use super::*;
use crate::redact::AdminRedactGuard;
use crate::resources::link_sys::{LinkSys, SystemType};
use k8s_openapi::api::core::v1::Secret;

#[test]
fn endpoint_grammar_and_ports() {
    for (raw, host, port, tls) in [
        ("http://collector", "collector", 80, false),
        ("https://Collector.example./", "Collector.example.", 443, true),
        ("http://192.0.2.1:4317", "192.0.2.1", 4317, false),
        ("https://[2001:db8::1]:4317/", "2001:db8::1", 4317, true),
        ("http://[::ffff:127.0.0.1]", "::ffff:127.0.0.1", 80, false),
    ] {
        let endpoint = parse_otlp_endpoint(raw).unwrap();
        assert_eq!((&*endpoint.host, endpoint.port, endpoint.tls), (host, port, tls));
        assert!(!format!("{endpoint:?}").contains(host));
    }
}

#[test]
fn endpoint_rejects_unsupported_raw_shapes_with_fixed_errors() {
    for raw in [
        "",
        "collector:4317",
        "ftp://collector",
        "http://",
        "http:///",
        "http://collector:",
        "http://collector:0",
        "http://collector:65536",
        "http://collector:+80",
        "http://user:password@collector",
        "http://collector?",
        "http://collector#",
        "http://collector/path/..",
        "http://collector//",
        "http://collector\\path",
        "http://collector/%2e",
        "http://collector\n",
        "http://col lector",
        "http://é.example",
        "http://[fe80::1%eth0]",
        "http://::1",
        "http://[::1]tail",
        "http://[bad]",
        "http://bad..name",
        "http://-bad",
        "http://bad-",
        "http://bad_name",
        "http://127.1",
        "http://2130706433",
        "http://999.0.0.1",
        "http://01.2.3.4",
    ] {
        assert_eq!(
            parse_otlp_endpoint(raw).unwrap_err(),
            "invalid OTLP endpoint",
            "{raw:?}"
        );
    }
}

fn config() -> OtlpClientConfig {
    serde_json::from_value(serde_json::json!({"endpoint":"https://collector"})).unwrap()
}

#[test]
fn validates_timeout_and_inert_tls() {
    let mut config = config();
    assert_eq!(config.timeout_ms, 10_000);
    for timeout in [0, 300_001, u64::MAX] {
        config.timeout_ms = timeout;
        assert_eq!(config.validate().unwrap_err(), "invalid OTLP timeout");
    }
    config.timeout_ms = 1;
    config.tls = Some(
        serde_json::from_value(serde_json::json!({"enabled":false,"verify":false,"validation":{"hostname":"ignored"}}))
            .unwrap(),
    );
    assert!(config.validate().is_ok());
    config.tls.as_mut().unwrap().enabled = true;
    assert!(config.validate().is_err());
    config.tls.as_mut().unwrap().outbound.verify = None;
    assert!(config.validate().is_err());
    config.tls.as_mut().unwrap().outbound.validation = None;
    assert!(config.validate().is_ok());
    config.endpoint = "http://collector".into();
    assert!(config.validate().is_err());
}

#[test]
fn secret_references_are_strict_and_disabled_tls_stays_inert() {
    for reference in [
        serde_json::json!({"name":"auth","kind":"ConfigMap"}),
        serde_json::json!({"name":"auth","group":"external.example"}),
        serde_json::json!({"name":"invalid/name"}),
        serde_json::json!({"name":"auth","namespace":"Invalid"}),
    ] {
        let config: OtlpClientConfig =
            serde_json::from_value(serde_json::json!({"endpoint":"https://collector","auth":{"secretRef":reference}}))
                .unwrap();
        assert_eq!(config.validate().unwrap_err(), "invalid OTLP Secret reference");
    }
    let mut config: OtlpClientConfig = serde_json::from_value(serde_json::json!({"endpoint":"https://collector","tls":{"enabled":true,"validation":{"caCertificateRefs":[{"kind":"ConfigMap","name":"ca"}]}}})).unwrap();
    assert_eq!(config.validate().unwrap_err(), "invalid OTLP Secret reference");
    config.tls.as_mut().unwrap().enabled = false;
    assert!(config.validate().is_ok());
}

#[test]
fn bounded_bearer_adapter_preserves_canonical_validation() {
    for length in [1, MAX_OTLP_BEARER_TOKEN_BYTES] {
        let data = BTreeMap::from([(BEARER_TOKEN_KEY.into(), ByteString(vec![b'a'; length]))]);
        assert_eq!(OtlpBearerResolver.parse(&data).unwrap().token.len(), length);
    }
    for value in [
        vec![b'a'; MAX_OTLP_BEARER_TOKEN_BYTES + 1],
        b"private value\r\n".to_vec(),
        vec![0xff],
    ] {
        let data = BTreeMap::from([(BEARER_TOKEN_KEY.into(), ByteString(value))]);
        let error = OtlpBearerResolver.parse(&data).unwrap_err();
        assert!(!format!("{error:?}").contains("private value"));
    }
    assert!(OtlpBearerResolver.parse(&BTreeMap::new()).is_err());
}

#[test]
fn schema_secret_transport_and_redaction() {
    let mut config = config();
    config.auth = Some(serde_json::from_value(serde_json::json!({"secretRef":{"name":"auth"}})).unwrap());
    let secret = Secret {
        data: Some(BTreeMap::from([(
            "bearerToken".into(),
            ByteString(b"private-token".to_vec()),
        )])),
        ..Default::default()
    };
    config.auth.as_mut().unwrap().slot.secret = Some(secret.clone());
    config.tls = Some(TlsConfig {
        enabled: true,
        resolved_ca_certificates: Some(vec![secret.clone()]),
        resolved_client_certificate: Some(secret),
        ..Default::default()
    });
    let transport = serde_json::to_value(&config).unwrap();
    assert!(transport["auth"]["secret"].is_object());
    let restored: OtlpClientConfig = serde_json::from_value(transport).unwrap();
    assert!(restored.auth.unwrap().slot.secret.is_some());
    let redacted = {
        let _guard = AdminRedactGuard::new();
        serde_json::to_value(&config).unwrap()
    };
    assert_eq!(redacted["auth"]["secret"], "[redacted]");
    assert_eq!(redacted["tls"]["resolvedCaCertificates"], "[redacted]");
    assert_eq!(redacted["tls"]["resolvedClientCertificate"], "[redacted]");
    let schema = serde_json::to_string(&schemars::schema_for!(OtlpClientConfig)).unwrap();
    assert!(!schema.contains("resolvedCaCertificates"));
    assert!(!schema.contains("resolvedClientCertificate"));
    assert!(!schema.contains("\"secret\""));
    assert!(!format!("{config:?}").contains("collector"));
}

#[test]
fn rejects_inline_fields_and_validates_resource_identity() {
    for field in ["protocol", "headers", "password", "token"] {
        let mut value = serde_json::json!({"endpoint":"http://collector"});
        value[field] = serde_json::json!("private");
        assert!(serde_json::from_value::<OtlpClientConfig>(value).is_err());
    }
    assert!(
        serde_json::from_value::<OtlpAuth>(serde_json::json!({"secretRef":{"name":"auth"},"token":"private"})).is_err()
    );
    let mut resource: LinkSys = serde_json::from_value(serde_json::json!({"apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"namespace":"default","name":"otlp"},"spec":{"type":"otlp","config":{"endpoint":"http://collector"}}})).unwrap();
    assert_eq!(resource.spec.config.system_type(), SystemType::Otlp);
    assert!(resource.validate_config().is_empty());
    resource.metadata.namespace = Some("private/invalid".into());
    assert_eq!(resource.validate_config(), ["invalid LinkSys reference"]);
    let crd: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../../../../../config/crd/edgion-crd/link_sys_crd.yaml")).unwrap();
    let discriminator =
        &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["type"];
    assert_eq!(discriminator["type"].as_str(), Some("string"));
    assert!(discriminator["enum"].is_null());
}

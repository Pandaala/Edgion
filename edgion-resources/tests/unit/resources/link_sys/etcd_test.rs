use super::super::{LinkSys, LinkSysSpec, SystemConfig, SystemType};
use super::*;

#[test]
fn test_deserialize_etcd_simple() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-simple
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "http://etcd-server:2379"
"#;

    let link_sys: LinkSys = yaml_serde::from_str(yaml).expect("Failed to deserialize");
    assert_eq!(link_sys.metadata.name.as_deref(), Some("etcd-simple"));
    assert_eq!(link_sys.metadata.namespace.as_deref(), Some("test"));
    assert_eq!(link_sys.spec.config.system_type(), SystemType::Etcd);

    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");
    assert_eq!(etcd_config.endpoints.len(), 1);
    assert_eq!(etcd_config.endpoints[0], "http://etcd-server:2379");
    assert!(etcd_config.auth.is_none());
    assert!(etcd_config.tls.is_none());
}

#[test]
fn test_deserialize_etcd_with_auth() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-auth
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd.example.com:2379"
    auth:
      secretRef:
        name: etcd-creds
        namespace: credentials
"#;

    let link_sys: LinkSys = yaml_serde::from_str(yaml).expect("Failed to deserialize");
    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");

    let auth = etcd_config.auth.as_ref().expect("Auth should be present");
    assert_eq!(auth.slot.secret_ref.name, "etcd-creds");
    assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("credentials"));
}

#[test]
fn test_deserialize_etcd_with_tls() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-tls
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd.example.com:2379"
    tls:
      enabled: true
      verify: false
      clientCertificateRef:
        name: etcd-client-tls
"#;

    let link_sys: LinkSys = yaml_serde::from_str(yaml).expect("Failed to deserialize");
    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");

    let tls = etcd_config.tls.as_ref().expect("TLS should be present");
    assert!(tls.enabled);
    assert_eq!(tls.outbound.verify, Some(false));
    assert!(tls.outbound.validation.is_none());
    assert_eq!(
        tls.outbound.client_certificate_ref.as_ref().map(|s| s.name.as_str()),
        Some("etcd-client-tls")
    );
}

#[test]
fn test_deserialize_etcd_with_tls_rejects_inline_client_key() {
    // A stray unknown field directly under `tls:` must be rejected by
    // `TlsConfig`'s `deny_unknown_fields`. Using `clientKey` preserves the
    // security intent: no inline private key material is accepted.
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-inline-key
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd.example.com:2379"
    tls:
      enabled: true
      clientKey: "INLINE_PRIVATE_KEY_SHOULD_FAIL"
"#;
    let r: Result<LinkSys, _> = yaml_serde::from_str(yaml);
    assert!(
        r.is_err(),
        "unknown TLS field must be rejected by TlsConfig's deny_unknown_fields"
    );
}

#[test]
fn test_deserialize_etcd_with_timeout() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-timeout
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "http://etcd:2379"
    timeout:
      dial: 5000
      request: 10000
"#;

    let link_sys: LinkSys = yaml_serde::from_str(yaml).expect("Failed to deserialize");
    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");

    let timeout = etcd_config.timeout.as_ref().expect("Timeout should be present");
    assert_eq!(timeout.dial, Some(5000));
    assert_eq!(timeout.request, Some(10000));
}

#[test]
fn test_deserialize_etcd_with_keepalive() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-with-keepalive
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "http://etcd:2379"
    keepAlive:
      time: 30
      timeout: 10
      permitWithoutStream: true
"#;

    let link_sys: LinkSys = yaml_serde::from_str(yaml).expect("Failed to deserialize");
    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");

    let keep_alive = etcd_config.keep_alive.as_ref().expect("KeepAlive should be present");
    assert_eq!(keep_alive.time, Some(30));
    assert_eq!(keep_alive.timeout, Some(10));
    assert_eq!(keep_alive.permit_without_stream, Some(true));
}

#[test]
fn test_deserialize_etcd_full_config() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-full
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd-1:2379"
      - "https://etcd-2:2379"
    auth:
      secretRef:
        name: etcd-creds
    tls:
      enabled: true
    timeout:
      dial: 5000
      request: 10000
    keepAlive:
      time: 30
      timeout: 10
    namespace: "/my-app/"
"#;

    let link_sys: LinkSys = yaml_serde::from_str(yaml).expect("Failed to deserialize");
    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");

    assert_eq!(etcd_config.endpoints.len(), 2);
    assert_eq!(etcd_config.namespace.as_deref(), Some("/my-app/"));
}

#[test]
fn test_serialize_etcd_config() {
    let link_sys = LinkSys {
        metadata: kube::core::ObjectMeta {
            name: Some("test-etcd".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Etcd(EtcdClientConfig {
                endpoints: vec!["http://etcd:2379".to_string()],
                auth: Some(EtcdAuth {
                    slot: crate::resources::common::SecretSlot {
                        secret_ref: crate::resources::common::SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "etcd-creds".to_string(),
                            namespace: None,
                        },
                        secret: None,
                    },
                }),
                tls: None,
                timeout: Some(EtcdTimeout {
                    dial: Some(5000),
                    request: Some(10000),
                }),
                keep_alive: None,
                namespace: Some("/app/".to_string()),
            }),
        },
        status: None,
    };

    let yaml = yaml_serde::to_string(&link_sys).expect("Failed to serialize");
    assert!(yaml.contains("type: etcd"));
    assert!(yaml.contains("endpoints:"));
    assert!(yaml.contains("http://etcd:2379"));
    assert!(yaml.contains("secretRef:"));
    assert!(yaml.contains("name: etcd-creds"));
    assert!(yaml.contains("namespace: /app/"));
}

#[test]
fn test_deserialize_etcd_with_secret_ref() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: etcd-secret
  namespace: test
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd:2379"
    auth:
      secretRef:
        name: etcd-creds
        namespace: test
    tls:
      enabled: true
"#;

    let link_sys: LinkSys = yaml_serde::from_str(yaml).expect("Failed to deserialize");
    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");

    let auth = etcd_config.auth.as_ref().expect("Auth should be present");
    assert_eq!(auth.slot.secret_ref.name, "etcd-creds");
    assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("test"));

    let tls = etcd_config.tls.as_ref().expect("TLS should be present");
    // The old caSource/caSecretRef fields are unknown to the new schema and are silently ignored
    assert!(tls.outbound.validation.is_none());
}

#[test]
fn etcd_auth_rejects_inline_password() {
    let yaml = "secretRef:\n  name: e\npassword: leaked";
    let r: Result<EtcdAuth, _> = yaml_serde::from_str(yaml);
    assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
}

#[test]
fn etcd_auth_rejects_inline_username() {
    let yaml = "secretRef:\n  name: e\nusername: admin";
    let r: Result<EtcdAuth, _> = yaml_serde::from_str(yaml);
    assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
}

#[test]
fn etcd_auth_yaml_roundtrip_unchanged() {
    let yaml = "secretRef:\n  name: etcd-creds\n  namespace: default";
    let auth: super::EtcdAuth = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(auth.slot.secret_ref.name, "etcd-creds");
    assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("default"));
    assert!(auth.slot.secret.is_none());
}

#[test]
fn etcd_auth_rejects_unknown_field() {
    let yaml = "secretRef:\n  name: x\nbogus: 1";
    let r: Result<super::EtcdAuth, _> = yaml_serde::from_str(yaml);
    assert!(r.is_err());
}

fn validated_config(extra: serde_json::Value) -> EtcdClientConfig {
    let mut value = serde_json::json!({"endpoints": ["http://etcd:2379"]});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

#[test]
fn etcd_endpoint_validation_normalizes_authorities_without_leaking_input() {
    for (value, normalized) in [
        ("http://ETCD:2379/", "http://etcd:2379"),
        ("https://etcd:443", "https://etcd"),
        ("http://[::1]:2379", "http://[::1]:2379"),
    ] {
        assert_eq!(normalize_etcd_endpoint(value).unwrap(), normalized);
    }
    for endpoint in [
        "etcd:2379",
        "ftp://etcd:2379",
        "http://",
        "http://etcd:0",
        "http://etcd:65536",
        "http://user:sentinel-secret@etcd:2379",
        "http://@etcd:2379",
        "http://etcd/path",
        "http://etcd/a/..",
        "http://etcd?query",
        "http://etcd#fragment",
        " http://etcd",
        "http://etcd\n",
        "http://etcd\\path",
    ] {
        assert!(normalize_etcd_endpoint(endpoint).is_err(), "{endpoint}");
        let errors = validated_config(serde_json::json!({"endpoints": [endpoint]})).validation_errors();
        assert!(!errors.is_empty());
        assert!(errors.iter().all(|error| !error.contains("sentinel-secret")));
    }
    assert!(normalize_etcd_endpoint(&format!("http://{}", "a".repeat(MAX_ETCD_ENDPOINT_BYTES))).is_err());
    assert!(
        validated_config(serde_json::json!({"endpoints": vec!["http://etcd:2379"; MAX_ETCD_ENDPOINTS]}))
            .validation_errors()
            .is_empty()
    );
    assert!(
        !validated_config(serde_json::json!({"endpoints": vec!["http://etcd:2379"; MAX_ETCD_ENDPOINTS + 1]}))
            .validation_errors()
            .is_empty()
    );
}

#[test]
fn etcd_keepalive_and_timeout_reject_partial_or_unbounded_settings() {
    for keep_alive in [
        serde_json::json!({}),
        serde_json::json!({"time": 1, "timeout": 86_400, "permitWithoutStream": false}),
    ] {
        assert!(validated_config(serde_json::json!({"keepAlive": keep_alive}))
            .validation_errors()
            .is_empty());
    }
    for keep_alive in [
        serde_json::json!({"time": 1}),
        serde_json::json!({"timeout": 1}),
        serde_json::json!({"permitWithoutStream": false}),
        serde_json::json!({"time": 0, "timeout": 1}),
        serde_json::json!({"time": 1, "timeout": 86_401}),
        serde_json::json!({"time": u64::MAX, "timeout": 1}),
    ] {
        assert!(!validated_config(serde_json::json!({"keepAlive": keep_alive}))
            .validation_errors()
            .is_empty());
    }
    for field in ["dial", "request"] {
        assert!(!validated_config(serde_json::json!({"timeout": {field: 0}}))
            .validation_errors()
            .is_empty());
        assert!(validated_config(serde_json::json!({"timeout": {field: 1}}))
            .validation_errors()
            .is_empty());
    }
}

#[test]
fn removed_etcd_options_are_unknown_and_not_serialized() {
    let value = serde_json::json!({
        "type": "etcd",
        "config": {
            "endpoints": ["http://etcd:2379"],
            "autoSyncInterval": 30,
            "maxCallSendSize": 1024,
            "maxCallRecvSize": 2048,
            "userAgent": "test-agent",
            "rejectOldCluster": true,
            "minClusterVersion": "3.5.0",
            "observability": {"metrics": {"enabled": true}},
            "timeout": {"keepAlive": 1000}
        }
    });
    let unknown = crate::schema_check::unknown_key_paths(&value, super::super::link_sys_spec_schema());
    for field in [
        "autoSyncInterval",
        "maxCallSendSize",
        "maxCallRecvSize",
        "userAgent",
        "rejectOldCluster",
        "minClusterVersion",
        "observability",
    ] {
        assert!(
            unknown.iter().any(|path| path == &format!("config.{field}")),
            "{field}: {unknown:?}"
        );
    }
    assert!(unknown.iter().any(|path| path == "config.timeout.keepAlive"));
    let spec: LinkSysSpec = serde_json::from_value(value).unwrap();
    let serialized = serde_json::to_value(spec).unwrap();
    assert!(serialized["config"].get("maxCallRecvSize").is_none());
    assert!(serialized["config"].get("observability").is_none());
}

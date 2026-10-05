use super::super::{LinkSys, LinkSysSpec, SystemConfig, SystemType};
use super::*;

#[test]
fn etcd_crd_exposes_policy_bounds_without_restricting_other_providers() {
    let crd: serde_json::Value =
        serde_yaml::from_str(include_str!("../../../../../config/crd/edgion-crd/link_sys_crd.yaml")).unwrap();
    let schema = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["config"];
    let validator = jsonschema::validator_for(schema).unwrap();
    for field in ["maxCallSendSize", "maxCallRecvSize"] {
        assert_eq!(schema["properties"][field]["type"], "integer");
        for (size, valid) in [(0, false), (1, true), (67_108_864, true), (67_108_865, false)] {
            let mut value = serde_json::json!({});
            value[field] = size.into();
            assert_eq!(validator.is_valid(&value), valid, "{field}: {size}");
        }
    }
    for (value, valid) in [
        (serde_json::json!({"autoSyncInterval": 0}), true),
        (serde_json::json!({"autoSyncInterval": 86400}), true),
        (serde_json::json!({"autoSyncInterval": 86401}), false),
        (serde_json::json!({"keepAlive": {"time": 0, "timeout": 1}}), false),
        (serde_json::json!({"keepAlive": {"time": 1, "timeout": 86401}}), false),
        (serde_json::json!({"keepAlive": {"time": 1, "timeout": 86400}}), true),
        (serde_json::json!({"minClusterVersion": "3.5.0"}), true),
        (serde_json::json!({"minClusterVersion": "03.5.0"}), false),
        (serde_json::json!({"minClusterVersion": "3.5.0-rc1"}), false),
        (serde_json::json!({"userAgent": "agent/v1"}), true),
        (serde_json::json!({"userAgent": "bad\nagent"}), false),
        (serde_json::json!({"userAgent": "a".repeat(257)}), false),
        (serde_json::json!({"endpoints": []}), true),
        (serde_json::json!({"endpoints": vec!["redis://cache:6379"; 65]}), true),
        (
            serde_json::json!({"observability": {"metrics": {"labels": {"provider": "other"}}}}),
            true,
        ),
    ] {
        assert_eq!(validator.is_valid(&value), valid, "{value}");
    }
}

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

    let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
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

    let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
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

    let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
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
    let r: Result<LinkSys, _> = serde_yaml::from_str(yaml);
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

    let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
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

    let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
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
    autoSyncInterval: 300
    maxCallSendSize: 2097152
    maxCallRecvSize: 4194304
    userAgent: "edgion-gateway/v1.0.0"
    rejectOldCluster: true
    minClusterVersion: "3.5.0"
    observability:
      logging:
        enabled: true
        level: "info"
        slowLogThreshold: 1000
      metrics:
        enabled: true
        labels: {}
"#;

    let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
    let etcd_config = link_sys.spec.config.as_etcd().expect("Etcd config should be present");

    assert_eq!(etcd_config.endpoints.len(), 2);
    assert_eq!(etcd_config.namespace.as_deref(), Some("/my-app/"));
    assert_eq!(etcd_config.auto_sync_interval, Some(300));
    assert_eq!(etcd_config.max_call_send_size, Some(2097152));
    assert_eq!(etcd_config.max_call_recv_size, Some(4194304));
    assert_eq!(etcd_config.user_agent.as_deref(), Some("edgion-gateway/v1.0.0"));
    assert_eq!(etcd_config.reject_old_cluster, Some(true));

    let observability = etcd_config
        .observability
        .as_ref()
        .expect("Observability should be present");
    let logging = observability.logging.as_ref().expect("Logging should be present");
    assert!(logging.enabled);
    assert_eq!(logging.level.as_deref(), Some("info"));
    assert_eq!(logging.slow_log_threshold, Some(1000));

    let metrics = observability.metrics.as_ref().expect("Metrics should be present");
    assert!(metrics.enabled);
    let labels = metrics.labels.as_ref().expect("Labels should be present");
    assert!(labels.is_empty());
    assert_eq!(etcd_config.parsed_min_cluster_version().unwrap(), Some([3, 5, 0]));
    assert!(link_sys.validate_config().is_empty());
}

#[test]
fn test_serialize_etcd_config() {
    let link_sys = LinkSys {
        metadata: kube::api::ObjectMeta {
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
                auto_sync_interval: Some(300),
                max_call_send_size: None,
                max_call_recv_size: None,
                user_agent: None,
                reject_old_cluster: None,
                min_cluster_version: None,
                observability: None,
            }),
        },
        status: None,
    };

    let yaml = serde_yaml::to_string(&link_sys).expect("Failed to serialize");
    assert!(yaml.contains("type: etcd"));
    assert!(yaml.contains("endpoints:"));
    assert!(yaml.contains("http://etcd:2379"));
    assert!(yaml.contains("secretRef:"));
    assert!(yaml.contains("name: etcd-creds"));
    assert!(yaml.contains("namespace: /app/"));
    assert!(yaml.contains("autoSyncInterval: 300"));
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

    let link_sys: LinkSys = serde_yaml::from_str(yaml).expect("Failed to deserialize");
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
    let r: Result<EtcdAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
}

#[test]
fn etcd_auth_rejects_inline_username() {
    let yaml = "secretRef:\n  name: e\nusername: admin";
    let r: Result<EtcdAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
}

#[test]
fn etcd_auth_yaml_roundtrip_unchanged() {
    let yaml = "secretRef:\n  name: etcd-creds\n  namespace: default";
    let auth: super::EtcdAuth = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(auth.slot.secret_ref.name, "etcd-creds");
    assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("default"));
    assert!(auth.slot.secret.is_none());
}

#[test]
fn etcd_auth_rejects_unknown_field() {
    let yaml = "secretRef:\n  name: x\nbogus: 1";
    let r: Result<super::EtcdAuth, _> = serde_yaml::from_str(yaml);
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
fn etcd_message_and_interval_boundaries() {
    for field in ["maxCallSendSize", "maxCallRecvSize"] {
        for value in [1, MAX_ETCD_MESSAGE_BYTES] {
            assert!(validated_config(serde_json::json!({field: value}))
                .validation_errors()
                .is_empty());
        }
        for value in [0, MAX_ETCD_MESSAGE_BYTES + 1, usize::MAX] {
            let errors = validated_config(serde_json::json!({field: value})).validation_errors();
            assert!(errors.iter().any(|error| error.contains(field)));
        }
    }
    for value in [0, 1, 86_400] {
        assert!(validated_config(serde_json::json!({"autoSyncInterval": value}))
            .validation_errors()
            .is_empty());
    }
    for value in [86_401, u64::MAX] {
        assert!(!validated_config(serde_json::json!({"autoSyncInterval": value}))
            .validation_errors()
            .is_empty());
    }
}

#[test]
fn etcd_version_policy_requires_an_explicit_stable_floor() {
    for version in [
        "3.5",
        "3.5.1.2",
        "3.5.1-rc1",
        "3.5.1+build",
        "03.5.1",
        "+3.5.1",
        " 3.5.1",
        "3..1",
        "18446744073709551616.0.0",
    ] {
        assert!(parse_etcd_version(version).is_err(), "{version}");
        assert!(
            !validated_config(serde_json::json!({"rejectOldCluster": true, "minClusterVersion": version}))
                .validation_errors()
                .is_empty()
        );
    }
    assert_eq!(parse_etcd_version("3.5.0").unwrap(), [3, 5, 0]);
    assert_eq!(parse_etcd_version("0.0.0").unwrap(), [0, 0, 0]);
    for extra in [
        serde_json::json!({"rejectOldCluster": true}),
        serde_json::json!({"minClusterVersion": "3.5.0"}),
        serde_json::json!({"rejectOldCluster": false, "minClusterVersion": "3.5.0"}),
    ] {
        assert!(!validated_config(extra).validation_errors().is_empty());
    }
    assert!(
        validated_config(serde_json::json!({"rejectOldCluster": true, "minClusterVersion": "3.5.0"}))
            .validation_errors()
            .is_empty()
    );
}

#[test]
fn etcd_user_agent_is_bounded_printable_ascii() {
    for agent in ["client/1".to_owned(), "a".repeat(256)] {
        assert!(validated_config(serde_json::json!({"userAgent": agent}))
            .validation_errors()
            .is_empty());
    }
    for agent in [
        "".to_owned(),
        " ".to_owned(),
        "a".repeat(257),
        "a\tb".to_owned(),
        "a\r\nb".to_owned(),
        "café".to_owned(),
    ] {
        assert!(!validated_config(serde_json::json!({"userAgent": agent}))
            .validation_errors()
            .is_empty());
    }
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
fn etcd_observability_rejects_arbitrary_labels_and_logging_contradictions() {
    for enabled in [true, false] {
        assert!(!validated_config(
            serde_json::json!({"observability": {"metrics": {"enabled": enabled, "labels": {"key": "value"}}}})
        )
        .validation_errors()
        .is_empty());
        assert!(validated_config(
            serde_json::json!({"observability": {"metrics": {"enabled": enabled, "labels": {}}}})
        )
        .validation_errors()
        .is_empty());
    }
    for level in ["error", "warn", "info", "debug", "trace"] {
        assert!(validated_config(
            serde_json::json!({"observability": {"logging": {"enabled": true, "level": level, "slowLogThreshold": 1}}})
        )
        .validation_errors()
        .is_empty());
    }
    for logging in [
        serde_json::json!({"level": "info"}),
        serde_json::json!({"slowLogThreshold": 1}),
        serde_json::json!({"enabled": true, "level": "INFO"}),
        serde_json::json!({"enabled": true, "slowLogThreshold": 0}),
    ] {
        assert!(
            !validated_config(serde_json::json!({"observability": {"logging": logging}}))
                .validation_errors()
                .is_empty()
        );
    }
}

#[test]
fn etcd_removed_timeout_is_unknown_and_version_floor_survives_serialization() {
    let value = serde_json::json!({"endpoints": ["http://etcd:2379"], "timeout": {"keepAlive": 1}, "rejectOldCluster": true, "minClusterVersion": "3.5.0"});
    let schema = schemars::schema_for!(EtcdClientConfig);
    assert_eq!(
        crate::schema_check::unknown_key_paths(&value, &schema),
        ["timeout.keepAlive"]
    );
    let config: EtcdClientConfig = serde_json::from_value(value).unwrap();
    let serialized = serde_json::to_value(config).unwrap();
    assert_eq!(serialized["minClusterVersion"], "3.5.0");
    assert!(serialized["timeout"].get("keepAlive").is_none());
}

#[test]
fn etcd_link_sys_validation_uses_canonical_field_rules() {
    let link: LinkSys = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "LinkSys", "metadata": {"name": "test"},
        "spec": {"type": "etcd", "config": {"endpoints": ["http://etcd:2379"], "maxCallRecvSize": 0}}
    }))
    .unwrap();
    assert!(link
        .validate_config()
        .iter()
        .any(|error| error.contains("maxCallRecvSize")));
}

use super::*;

#[test]
fn health_check_port_validation_preserves_inheritance_and_checks_all_protocols() {
    for protocol in ["http", "https", "tcp", "grpc", "grpcs"] {
        for (fields, expected_port, expected_error) in [
            (serde_json::json!({}), None, None),
            (serde_json::json!({"port": null}), None, None),
            (serde_json::json!({"port": 0}), Some(0), Some("port must be >= 1")),
            (serde_json::json!({"port": 1}), Some(1), None),
            (serde_json::json!({"port": 65535}), Some(65535), None),
        ] {
            let mut active = fields;
            active["type"] = serde_json::json!(protocol);
            let document = serde_json::json!({"active": active});
            let json: ServiceHealthCheck = serde_json::from_value(document.clone()).unwrap();
            let yaml: ServiceHealthCheck = yaml_serde::from_str(&yaml_serde::to_string(&document).unwrap()).unwrap();
            for config in [json, yaml] {
                assert_eq!(config.active.as_ref().unwrap().port, expected_port, "{document}");
                assert_eq!(config.get_validation_error(), expected_error, "{document}");
            }
        }
    }
    assert!(ServiceHealthCheck { active: None }.get_validation_error().is_none());
}

#[test]
fn health_check_port_rejects_values_outside_the_unsigned_representation() {
    for port in [serde_json::json!(-1), serde_json::json!(65536)] {
        let document = serde_json::json!({"active": {"port": port}});
        assert!(serde_json::from_value::<ServiceHealthCheck>(document.clone()).is_err());
        assert!(yaml_serde::from_str::<ServiceHealthCheck>(&yaml_serde::to_string(&document).unwrap()).is_err());
    }
}

#[test]
fn test_deserialize_minimal_config() {
    let yaml = r#"
active:
  type: http
  path: /healthz
"#;

    let cfg: ServiceHealthCheck = yaml_serde::from_str(yaml).expect("deserialize minimal config");
    let active = cfg.active.expect("active config");
    assert_eq!(active.r#type, HealthCheckType::Http);
    assert_eq!(active.path.as_deref(), Some("/healthz"));
    assert_eq!(active.interval, "10s");
    assert_eq!(active.timeout, "3s");
    assert_eq!(active.healthy_threshold, 2);
    assert_eq!(active.unhealthy_threshold, 3);
    assert_eq!(active.expected_statuses, vec![200]);
}

#[test]
fn test_deserialize_full_config() {
    let yaml = r#"
active:
  type: tcp
  path: /ignored
  port: 9090
  interval: 5s
  timeout: 1s
  healthyThreshold: 3
  unhealthyThreshold: 5
  expectedStatuses: [200, 204]
  host: backend.internal
"#;

    let cfg: ServiceHealthCheck = yaml_serde::from_str(yaml).expect("deserialize full config");
    let active = cfg.active.expect("active config");
    assert_eq!(active.r#type, HealthCheckType::Tcp);
    assert_eq!(active.port, Some(9090));
    assert_eq!(active.interval, "5s");
    assert_eq!(active.timeout, "1s");
    assert_eq!(active.healthy_threshold, 3);
    assert_eq!(active.unhealthy_threshold, 5);
    assert_eq!(active.expected_statuses, vec![200, 204]);
    assert_eq!(active.host.as_deref(), Some("backend.internal"));
}

#[test]
fn test_default_values() {
    let cfg = ActiveHealthCheckConfig::default();
    assert_eq!(cfg.r#type, HealthCheckType::Http);
    assert_eq!(cfg.path.as_deref(), Some("/"));
    assert_eq!(cfg.interval, "10s");
    assert_eq!(cfg.timeout, "3s");
    assert_eq!(cfg.healthy_threshold, 2);
    assert_eq!(cfg.unhealthy_threshold, 3);
    assert_eq!(cfg.expected_statuses, vec![200]);
}

#[test]
fn test_deserialize_grpc_config() {
    let yaml = r#"
active:
  type: grpc
  port: 9090
  interval: 10s
  timeout: 3s
  healthyThreshold: 2
  unhealthyThreshold: 3
  grpcServiceName: "my.package.MyService"
"#;

    let cfg: ServiceHealthCheck = yaml_serde::from_str(yaml).expect("deserialize grpc config");
    assert!(cfg.get_validation_error().is_none());
    let active = cfg.active.expect("active config");
    assert_eq!(active.r#type, HealthCheckType::Grpc);
    assert_eq!(active.port, Some(9090));
    assert_eq!(active.grpc_service_name.as_deref(), Some("my.package.MyService"));
}

#[test]
fn test_grpc_config_without_service_name() {
    let yaml = r#"
active:
  type: grpc
  port: 9090
"#;

    let cfg: ServiceHealthCheck = yaml_serde::from_str(yaml).expect("deserialize grpc config");
    assert!(cfg.get_validation_error().is_none());
    let active = cfg.active.expect("active config");
    assert_eq!(active.r#type, HealthCheckType::Grpc);
    assert!(active.grpc_service_name.is_none());
}

#[test]
fn test_validation_rejects_invalid_config() {
    let yaml = r#"
active:
  type: http
  path: ""
  interval: 0s
  timeout: 3s
  healthyThreshold: 0
"#;

    let cfg: ServiceHealthCheck = yaml_serde::from_str(yaml).expect("deserialize invalid config");
    assert!(cfg.get_validation_error().is_some());
}

#[test]
fn test_serde_roundtrip() {
    let original = ServiceHealthCheck {
        active: Some(ActiveHealthCheckConfig {
            r#type: HealthCheckType::Http,
            path: Some("/livez".to_string()),
            port: Some(8080),
            interval: "7s".to_string(),
            timeout: "2s".to_string(),
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            expected_statuses: vec![200, 204],
            host: Some("svc.local".to_string()),
            grpc_service_name: None,
            tls: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            resolved_tls_error: None,
        }),
    };

    let yaml = yaml_serde::to_string(&original).expect("serialize");
    let decoded: ServiceHealthCheck = yaml_serde::from_str(&yaml).expect("deserialize");
    assert_eq!(decoded.active, original.active);
}
#[test]
fn encrypted_tls_validation_is_independent_of_attachment() {
    for protocol in ["https", "grpcs"] {
        let mut active: ActiveHealthCheckConfig = serde_json::from_value(serde_json::json!({
            "type": protocol, "tls": {"validation": {"hostname": "health.example.com"}}
        }))
        .unwrap();
        assert!(active.tls_validation_error().is_none());
        assert!(active.tls_namespace_validation_error("ns").is_none());
        for hostname in [
            "127.0.0.1",
            "127.1",
            "2130706433",
            "0x7f000001",
            "0177.0.0.1",
            "*.example.com",
            "bad host",
        ] {
            active.tls.as_mut().unwrap().validation.as_mut().unwrap().hostname = Some(hostname.into());
            assert!(active.tls_validation_error().is_some(), "hostname: {hostname}");
            assert!(ServiceHealthCheck {
                active: Some(active.clone())
            }
            .get_validation_error()
            .is_none());
        }
        active.tls = None;
        assert!(active.tls_validation_error().is_some());
    }
    // Existing plaintext protocols remain literal, even with an irrelevant TLS block.
    for protocol in ["http", "tcp", "grpc"] {
        let active: ActiveHealthCheckConfig =
            serde_json::from_value(serde_json::json!({"type": protocol, "tls": {"verify": false}})).unwrap();
        assert!(active.tls_validation_error().is_none());
    }
}

#[test]
fn encrypted_tls_rejects_invalid_trust_reference_and_san_shapes() {
    for tls in [
        serde_json::json!({"verify": false, "validation": {"hostname": "health.example.com"}}),
        serde_json::json!({"validation": {"hostname": "health.example.com", "caCertificateRefs": []}}),
        serde_json::json!({"validation": {"hostname": "health.example.com", "caCertificateRefs": [{"kind": "Secret", "name": "ca"}], "wellKnownCACertificates": "System"}}),
        serde_json::json!({"validation": {"hostname": "health.example.com", "caCertificateRefs": [{"group": "foreign.io", "kind": "Secret", "name": "ca"}]}}),
        serde_json::json!({"validation": {"hostname": "health.example.com", "subjectAltNames": [{"type": "Hostname", "uri": "spiffe://example/health"}]}}),
        serde_json::json!({"validation": {"hostname": "health.example.com"}, "clientCertificateRef": {"kind": "ConfigMap", "name": "identity"}}),
    ] {
        let active: ActiveHealthCheckConfig =
            serde_json::from_value(serde_json::json!({"type": "https", "tls": tls})).unwrap();
        assert!(active.tls_validation_error().is_some());
    }
    let active: ActiveHealthCheckConfig = serde_json::from_value(serde_json::json!({"type": "https", "tls": {
            "validation": {"hostname": "health.example.com", "caCertificateRefs": [{"kind": "ConfigMap", "name": "ca", "namespace": "other"}]}
        }})).unwrap();
    assert!(active.tls_validation_error().is_none());
    assert!(active.tls_namespace_validation_error("ns").is_some());
    assert!(active.tls_namespace_validation_error("other").is_none());
}

#[test]
fn encrypted_tls_material_is_redacted_and_not_operator_schema() {
    let secret: Secret = serde_json::from_value(serde_json::json!({"data": {"tls.key": "c2VjcmV0"}})).unwrap();
    let active = ActiveHealthCheckConfig {
        resolved_ca_certificates: Some(vec![secret.clone()]),
        resolved_client_certificate: Some(secret),
        resolved_tls_error: Some("health check CA reference is unresolved".into()),
        ..Default::default()
    };
    let schema = serde_json::to_value(schemars::schema_for!(ActiveHealthCheckConfig)).unwrap();
    for key in [
        "resolvedCaCertificates",
        "resolvedClientCertificate",
        "resolvedTlsError",
    ] {
        assert!(schema["properties"].get(key).is_none());
    }
    assert!(schema["properties"].get("tls").is_some());
    let sync = serde_json::to_value(&active).unwrap();
    assert_eq!(sync["resolvedClientCertificate"]["data"]["tls.key"], "c2VjcmV0");
    let decoded: ActiveHealthCheckConfig = serde_json::from_value(sync).unwrap();
    assert_eq!(decoded, active);
    let admin = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_value(&active).unwrap()
    };
    assert_eq!(admin["resolvedClientCertificate"], "[redacted]");
    assert_eq!(admin["resolvedCaCertificates"], "[redacted]");
    assert!(!format!("{active:?}").contains("c2VjcmV0"));
    let mut changed = active.clone();
    changed.resolved_client_certificate = None;
    assert_ne!(changed, active, "identity rotation must affect generation equality");
}

#[test]
fn encrypted_tls_reference_names_preserve_health_length_and_syntax_boundaries() {
    let label63 = "a".repeat(63);
    let name253 = format!("{label63}.{label63}.{label63}.{}", "a".repeat(61));
    let name254 = format!("{name253}a");
    let cases = [
        ("a-1".to_string(), true, true),
        ("123".to_string(), true, true),
        ("ca.example".to_string(), true, false),
        (label63, true, true),
        ("a".repeat(64), false, false),
        (name253, true, false),
        (name254, false, false),
        (String::new(), false, false),
        ("Upper".to_string(), false, false),
        ("under_score".to_string(), false, false),
        ("café".to_string(), false, false),
        ("-edge".to_string(), false, false),
        ("edge-".to_string(), false, false),
        (".edge".to_string(), false, false),
        ("edge.".to_string(), false, false),
        ("empty..label".to_string(), false, false),
    ];
    for protocol in ["https", "grpcs"] {
        for kind in ["Secret", "ConfigMap", "client"] {
            for field in ["name", "namespace"] {
                for (value, valid_name, valid_namespace) in &cases {
                    let mut reference = serde_json::json!({"kind": kind, "name": "ca", "namespace": "ns"});
                    reference[field] = serde_json::json!(value);
                    let mut tls = serde_json::json!({"validation": {"hostname": "health.example.com"}});
                    let error = if kind == "client" {
                        reference["kind"] = serde_json::json!("Secret");
                        tls["clientCertificateRef"] = reference;
                        "health check client certificate reference is invalid"
                    } else {
                        tls["validation"]["caCertificateRefs"] = serde_json::json!([reference]);
                        "health check CA reference is invalid"
                    };
                    let active: ActiveHealthCheckConfig =
                        serde_json::from_value(serde_json::json!({"type": protocol, "tls": tls})).unwrap();
                    let valid = if field == "name" { *valid_name } else { *valid_namespace };
                    assert_eq!(
                        active.tls_validation_error(),
                        (!valid).then_some(error),
                        "{protocol} {kind} {field}={value:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn encrypted_tls_hostname_preserves_component_and_total_bounds() {
    let label63 = "a".repeat(63);
    let hostname253 = format!("{label63}.{label63}.{label63}.{}", "a".repeat(61));
    for protocol in ["https", "grpcs"] {
        for (hostname, expected) in [
            ("a-1.example".to_string(), None),
            (
                String::new(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                "Upper".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                "under_score".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                "café".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                "-edge".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                "edge-".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                ".edge".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                "edge.".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                "empty..label".to_string(),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (label63.clone(), None),
            ("a".repeat(64), Some("health check TLS hostname must be a DNS name")),
            (hostname253.clone(), None),
            (
                format!("{hostname253}a"),
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
        ] {
            let active: ActiveHealthCheckConfig = serde_json::from_value(serde_json::json!({
                "type": protocol, "tls": {"validation": {"hostname": hostname}}
            }))
            .unwrap();
            assert_eq!(active.tls_validation_error(), expected, "{protocol}: {hostname}");
        }
    }
}

#[test]
fn encrypted_tls_validation_preserves_error_precedence() {
    for protocol in ["https", "grpcs"] {
        let mut document = serde_json::json!({"type": protocol, "tls": {
            "verify": false,
            "validation": {"hostname": "*.example.com", "caCertificateRefs": [{"kind": "Secret", "name": "bad_name"}]},
            "clientCertificateRef": {"name": "bad_name"}
        }});
        for (verify, hostname, ca_name, expected) in [
            (
                false,
                "*.example.com",
                "bad_name",
                Some("encrypted health checks require certificate verification"),
            ),
            (
                true,
                "*.example.com",
                "bad_name",
                Some("health check TLS hostname or subjectAltNames is invalid"),
            ),
            (
                true,
                "2130706433",
                "bad_name",
                Some("health check TLS hostname must be a DNS name"),
            ),
            (
                true,
                "health.example.com",
                "bad_name",
                Some("health check CA reference is invalid"),
            ),
            (
                true,
                "health.example.com",
                "ca",
                Some("health check client certificate reference is invalid"),
            ),
        ] {
            document["tls"]["verify"] = serde_json::json!(verify);
            document["tls"]["validation"]["hostname"] = serde_json::json!(hostname);
            document["tls"]["validation"]["caCertificateRefs"][0]["name"] = serde_json::json!(ca_name);
            let active: ActiveHealthCheckConfig = serde_json::from_value(document.clone()).unwrap();
            assert_eq!(active.tls_validation_error(), expected, "{document}");
        }
    }
}

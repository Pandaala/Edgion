//! Unit tests for `webhook.rs` (see `super`).
use super::*;
use crate::resources::common::{validate_webhook_target, CustomField, CustomSource, PassiveHealthCheck, WebhookTarget};

#[test]
fn test_default_config() {
    let config = WebhookServiceConfig::default();
    assert_eq!(config.conn.timeout_ms, 5000);
    assert_eq!(config.conn.max_response_bytes, 1024);
    assert_eq!(config.conn.status_on_error, 503);
    assert!(config.conn.retry.is_none());
    assert!(config.conn.rate_limit.is_none());
    assert!(config.conn.health_check.is_none());
    // Default target has no port (will fail validation)
    assert!(config.conn.target.service.port.is_none());
}

#[test]
fn test_validation_empty_default() {
    let config = WebhookServiceConfig::default();
    // Default has neither a url nor any service field set, so it fails validation
    // with the mutual-exclusion check (Task 1) rather than a missing-port error.
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("target: one of url or name is required")
    );
}

// URI variant removed; WebhookTarget now only supports Service backends

#[test]
fn test_validation_zero_timeout() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("svc".to_string()),
                    namespace: None,
                    port: Some(80),
                    ..Default::default()
                },
                ..Default::default()
            },
            timeout_ms: 0,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("timeoutMs must be between 1 and 60000")
    );
}

#[test]
fn test_validation_valid_config() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("api-svc".to_string()),
                    namespace: Some("prod".to_string()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(config.get_validation_error().is_none());
}

// ---- Service target validation ----

#[test]
fn test_validation_service_no_port() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("my-svc".to_string()),
                    namespace: None,
                    port: None,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("backendRef port is required")
    );
}

#[test]
fn test_validation_service_port_zero() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("my-svc".to_string()),
                    namespace: None,
                    port: Some(0),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("backendRef port must be in 1..=65535")
    );
}

#[test]
fn test_validation_service_port_above_max() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("my-svc".to_string()),
                    namespace: None,
                    port: Some(70000),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("backendRef port must be in 1..=65535")
    );
}

#[test]
fn test_validation_service_port_negative() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("my-svc".to_string()),
                    namespace: None,
                    port: Some(-1),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("backendRef port must be in 1..=65535")
    );
}

#[test]
fn test_validation_service_valid() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("my-svc".to_string()),
                    namespace: Some("prod".to_string()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_validation_service_kind_service_import() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: Some("ServiceImport".to_string()),
                    name: Some("my-svc".to_string()),
                    namespace: None,
                    port: Some(80),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("ServiceImport is not yet supported for webhook")
    );
}

#[test]
fn test_validation_service_kind_external_name() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: Some("ServiceExternalName".to_string()),
                    name: Some("my-svc".to_string()),
                    namespace: None,
                    port: Some(80),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("ServiceExternalName is not supported for webhook")
    );
}

#[test]
fn test_validation_service_kind_unsupported() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: Some("BogusKind".to_string()),
                    name: Some("my-svc".to_string()),
                    namespace: None,
                    port: Some(80),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("unsupported service kind: BogusKind")
    );
}

#[test]
fn test_serde_minimal_yaml() {
    let yaml = r#"
target:
  name: "api-svc"
  port: 8080
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.conn.target.service.name.as_deref(), Some("api-svc"));
    assert_eq!(config.conn.target.service.port, Some(8080));
    assert_eq!(config.conn.timeout_ms, 5000);
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_serde_full_yaml() {
    let yaml = r#"
target:
  name: "key-service"
  namespace: "internal"
  port: 443
  kind: "Service"
timeoutMs: 3000
success:
  statusCodes: [200]
maxResponseBytes: 2048
statusOnError: 503
retry:
  maxRetries: 1
  retryDelayMs: 100
  retryOnTimeout: true
  retryOnConnectError: true
  retryOnStatus: [502, 503, 504]
rateLimit:
  rate: 200
  windowSec: 1
healthCheck:
  passive:
    unhealthyThreshold: 3
    failureStatusCodes: [500, 502, 503, 504]
    countTimeout: true
  active:
    path: "/healthz"
    intervalSec: 10
    timeoutMs: 2000
    healthyThreshold: 1
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.conn.target.service.name.as_deref(), Some("key-service"));
    assert_eq!(config.conn.target.service.namespace, Some("internal".to_string()));
    assert_eq!(config.conn.target.service.port, Some(443));
    assert_eq!(config.conn.timeout_ms, 3000);
    assert_eq!(config.conn.max_response_bytes, 2048);
    let success = config.conn.success.as_ref().expect("success block must deserialize");
    assert_eq!(success.status_codes, Some(vec![200]));
    // Validate before consuming fields with unwrap()
    assert!(config.get_validation_error().is_none());
    assert!(config.conn.retry.is_some());
    let retry = config.conn.retry.unwrap();
    assert_eq!(retry.max_retries, 1);
    assert_eq!(retry.retry_on_timeout, Some(true));
    assert_eq!(retry.retry_on_status, vec![502, 503, 504]);
    assert!(config.conn.rate_limit.is_some());
    let rl = config.conn.rate_limit.unwrap();
    assert_eq!(rl.rate, 200);
    assert_eq!(rl.window_sec, 1);
    assert!(config.conn.health_check.is_some());
    let hc = config.conn.health_check.unwrap();
    assert!(hc.passive.is_some());
    assert!(hc.active.is_some());
}

#[test]
fn test_serde_service_target_yaml() {
    let yaml = r#"
target:
  name: "my-webhook-svc"
  namespace: "prod"
  port: 8080
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.conn.target.service.name.as_deref(), Some("my-webhook-svc"));
    assert_eq!(config.conn.target.service.namespace, Some("prod".to_string()));
    assert_eq!(config.conn.target.service.port, Some(8080));
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_serde_roundtrip() {
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: None,
                    name: Some("example-svc".to_string()),
                    namespace: None,
                    port: Some(443),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: WebhookServiceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.conn.target.service.name, config.conn.target.service.name);
    assert_eq!(deserialized.conn.target.service.port, config.conn.target.service.port);
    assert_eq!(deserialized.conn.timeout_ms, config.conn.timeout_ms);
}

#[test]
fn test_serde_service_target_roundtrip() {
    // Verifies both serialization and deserialization of WebhookServiceRef,
    // including optional fields.
    let config = WebhookServiceConfig {
        conn: ExternalHttpConnection {
            target: WebhookTarget {
                service: WebhookServiceRef {
                    kind: Some("ServiceClusterIp".to_string()),
                    name: Some("auth-svc".to_string()),
                    namespace: Some("prod".to_string()),
                    port: Some(8080),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    // Verify None/empty fields are omitted.
    assert!(!json.contains("\"group\""), "empty group must be omitted: {json}");

    let deserialized: WebhookServiceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.conn.target.service.name.as_deref(), Some("auth-svc"));
    assert_eq!(deserialized.conn.target.service.namespace.as_deref(), Some("prod"));
    assert_eq!(deserialized.conn.target.service.port, Some(8080));
    assert_eq!(
        deserialized.conn.target.service.kind.as_deref(),
        Some("ServiceClusterIp")
    );
}

#[test]
fn service_target_ignores_unrecognized_fields() {
    // Task 1 (WebhookTarget URL variant): every field on WebhookTarget /
    // WebhookServiceRef is now optional, and `#[serde(flatten)]` disables
    // `deny_unknown_fields` regardless (spec §3.2). A malformed / unrecognized
    // shape like a nested "backendRef" key therefore no longer fails to
    // deserialize -- it just doesn't populate anything.
    let json = r#"{"backendRef":{"name":"svc","port":80},"extraField":"ignored"}"#;
    let r: Result<WebhookTarget, _> = serde_json::from_str(json);
    assert!(r.is_ok(), "unrecognized fields are now silently ignored: {:?}", r);
}

#[test]
fn test_passive_backoff_defaults() {
    let yaml = r#"
unhealthyThreshold: 5
countTimeout: true
backoff:
  initialSec: 5
"#;
    let passive: PassiveHealthCheck = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(passive.unhealthy_threshold, 5);
    assert!(passive.count_timeout);
    let backoff = passive.backoff.unwrap();
    assert_eq!(backoff.initial_sec, 5);
    assert!((backoff.multiplier - 2.0).abs() < f64::EPSILON);
    assert_eq!(backoff.max_sec, 60);
}

// ============================================================
// WebhookTarget struct tests (Task A1)
// ============================================================

#[test]
fn test_webhook_target_valid_service_with_port() {
    // Valid: Service with required port
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some("Service".to_string()),
            name: Some("my-svc".to_string()),
            namespace: Some("prod".to_string()),
            port: Some(8080),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "valid service should not have error: {:?}", error);
}

#[test]
fn test_webhook_target_missing_port_error() {
    // Invalid: port is None (required)
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some("Service".to_string()),
            name: Some("my-svc".to_string()),
            namespace: Some("prod".to_string()),
            port: None,
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_some(), "missing port should produce error");
    assert_eq!(error.unwrap(), "backendRef port is required");
}

#[test]
fn test_webhook_target_external_name_rejected() {
    // Invalid: ServiceExternalName kind is not allowed
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some("ServiceExternalName".to_string()),
            name: Some("external-svc".to_string()),
            namespace: None,
            port: Some(443),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_some(), "ServiceExternalName should be rejected");
    assert_eq!(error.unwrap(), "ServiceExternalName is not supported for webhook");
}

#[test]
fn test_webhook_target_service_cluster_ip_valid() {
    // Valid: ServiceClusterIp is in allowed set
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some("ServiceClusterIp".to_string()),
            name: Some("cluster-svc".to_string()),
            namespace: Some("default".to_string()),
            port: Some(3000),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "ServiceClusterIp should be valid: {:?}", error);
}

#[test]
fn test_webhook_target_service_endpoint_slice_valid() {
    // Valid: ServiceEndpointSlice is in allowed set
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some("ServiceEndpointSlice".to_string()),
            name: Some("endpoint-svc".to_string()),
            namespace: None,
            port: Some(9090),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "ServiceEndpointSlice should be valid: {:?}", error);
}

#[test]
fn test_webhook_target_kind_empty_string_valid() {
    // Valid: empty kind defaults to Service
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some(String::new()),
            name: Some("my-svc".to_string()),
            namespace: None,
            port: Some(80),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "empty kind should default to Service: {:?}", error);
}

#[test]
fn test_webhook_target_kind_none_valid() {
    // Valid: None kind defaults to Service
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: None,
            name: Some("my-svc".to_string()),
            namespace: None,
            port: Some(80),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "None kind should default to Service: {:?}", error);
}

#[test]
fn test_webhook_target_unsupported_kind_rejected() {
    // Invalid: unsupported kind
    let target = WebhookTarget {
        service: WebhookServiceRef {
            kind: Some("UnsupportedKind".to_string()),
            name: Some("unknown-svc".to_string()),
            namespace: None,
            port: Some(80),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_some(), "unsupported kind should be rejected");
    assert_eq!(error.unwrap(), "unsupported service kind: UnsupportedKind");
}

/// Intentional: #[serde(flatten)] disables deny_unknown_fields (spec §3.2).
/// Unknown fields in a flattened WebhookServiceConfig are now silently ignored
/// rather than rejected. This documents the intentional relaxation.
#[test]
fn webhook_config_ignores_unknown_fields_after_flatten() {
    // Intentional: #[serde(flatten)] disables deny_unknown_fields (spec §3.2).
    let y = "target:\n  name: s\n  port: 1\nunknownFieldName: x\n";
    assert!(serde_yaml::from_str::<WebhookServiceConfig>(y).is_ok());
}

#[test]
fn validation_rejects_bare_secret_ref() {
    // A bare `${secretRef}` (no ns/name:key) in a custom template header
    // must be rejected at validation — it would produce an empty credential.
    let mut config = WebhookServiceConfig::default();
    config.conn.target.service.name = Some("webhook-svc".to_string());
    config.conn.target.service.port = Some(8080);
    config.request.headers.custom.push(CustomField {
        name: "Authorization".to_string(),
        source: CustomSource::Template("Bearer ${secretRef}".to_string()),
    });
    let err = config.get_validation_error().expect("must reject bare secretRef");
    assert!(err.contains("secretRef"), "got: {err}");
}

#[test]
fn test_webhook_config_without_shim_fields_valid() {
    // Config without any shim fields should be valid
    let yaml = r#"
target:
  name: "api-svc"
  port: 8080
timeoutMs: 3000
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.conn.target.service.port.is_some());
    assert_eq!(config.conn.timeout_ms, 3000);
}

// ============================================================
// Task 2 tests
// ============================================================

/// After the flatten refactor, `target` must still parse as a nested object
/// (not flattened). This tests that `cfg.conn.target.service.name` is
/// accessible after deserializing from the flat YAML representation.
#[test]
fn webhook_config_target_stays_nested_after_flatten() {
    let y = "target:\n  name: svc\n  namespace: prod\n  port: 8080\nrequest: {}\n";
    let cfg: WebhookServiceConfig = serde_yaml::from_str(y).unwrap();
    assert_eq!(cfg.conn.target.service.name.as_deref(), Some("svc"));
    assert_eq!(cfg.conn.target.service.port, Some(8080));
}

// ============================================================
// Task 1 tests: WebhookTarget URL variant
// ============================================================

use crate::resources::common::external_http::{TargetKind, WebhookServiceRef};

/// A bare URL target parses and derives TargetKind::Url.
#[test]
fn test_target_url_variant_parses() {
    let target: WebhookTarget = serde_yaml::from_str("url: https://api.example.com").unwrap();
    match target.kind().expect("url target must be valid") {
        TargetKind::Url { url, block_private } => {
            assert_eq!(url, "https://api.example.com");
            assert_eq!(block_private, None);
        }
        TargetKind::Service(_) => panic!("expected Url kind"),
    }
}

/// url + service fields is a hard error, NOT a silent fall-through to Service.
/// This is the exact failure mode an untagged enum would have had (spec 2.5).
#[test]
fn test_target_url_and_service_fields_are_mutually_exclusive() {
    let target: WebhookTarget = serde_yaml::from_str("url: https://api.example.com\nname: svc\nport: 8080").unwrap();
    let err = target.kind().expect_err("url + name must be rejected");
    assert!(err.contains("mutually exclusive"), "got: {err}");
}

/// Neither url nor name.
#[test]
fn test_target_empty_is_rejected() {
    let target = WebhookTarget::default();
    let err = target.kind().expect_err("empty target must be rejected");
    assert!(err.contains("one of url or name is required"), "got: {err}");
}

/// Origin-only: a path is rejected.
#[test]
fn test_target_url_with_path_is_rejected() {
    let target: WebhookTarget = serde_yaml::from_str("url: https://api.example.com/v1/check").unwrap();
    let err = target.kind().expect_err("url with path must be rejected");
    assert!(err.contains("origin-only"), "got: {err}");
}

#[test]
fn test_target_url_with_query_is_rejected() {
    let target: WebhookTarget = serde_yaml::from_str("url: https://api.example.com?a=1").unwrap();
    let err = target.kind().expect_err("url with query must be rejected");
    assert!(err.contains("origin-only"), "got: {err}");
}

#[test]
fn test_target_url_scheme_must_be_http_or_https() {
    let target: WebhookTarget = serde_yaml::from_str("url: ftp://api.example.com").unwrap();
    let err = target.kind().expect_err("ftp must be rejected");
    assert!(err.contains("scheme must be http or https"), "got: {err}");
}

#[test]
fn test_target_url_with_credentials_is_rejected() {
    let target: WebhookTarget = serde_yaml::from_str("url: https://user:pw@api.example.com").unwrap();
    let err = target.kind().expect_err("credentials must be rejected");
    assert!(err.contains("credentials"), "got: {err}");
}

/// An explicit port and an IPv6 literal are both valid origins.
#[test]
fn test_target_url_accepts_port_and_ipv6_literal() {
    for raw in ["url: https://api.example.com:8443", "url: http://[2001:db8::1]:8080"] {
        let target: WebhookTarget = serde_yaml::from_str(raw).unwrap();
        assert!(target.kind().is_ok(), "{raw} should be a valid origin");
    }
}

/// blockPrivate is meaningless on a Service target.
#[test]
fn test_block_private_on_service_target_is_rejected() {
    let target: WebhookTarget = serde_yaml::from_str("name: svc\nport: 8080\nblockPrivate: true").unwrap();
    let err = target
        .kind()
        .expect_err("blockPrivate on a service target must be rejected");
    assert!(err.contains("blockPrivate applies to url targets only"), "got: {err}");
}

/// http:// with tls.enabled must not silently downgrade to plaintext.
#[test]
fn test_http_url_with_tls_enabled_is_rejected() {
    use crate::resources::common::external_http::validate_connection_target;
    use crate::resources::common::TlsConfig;

    let mut conn = ExternalHttpConnection {
        target: serde_yaml::from_str("url: http://api.example.com").unwrap(),
        ..Default::default()
    };
    conn.tls = Some(TlsConfig {
        enabled: true,
        ..Default::default()
    });
    let err = validate_connection_target(&conn).expect("http + tls.enabled must be rejected");
    assert!(err.contains("tls.enabled"), "got: {err}");
}

/// The plaintext-downgrade guard must be case-insensitive: `url::Url` normalizes
/// the scheme, so a raw-prefix check would silently let `HTTP://` through.
#[test]
fn test_uppercase_http_url_with_tls_enabled_is_rejected() {
    use crate::resources::common::external_http::validate_connection_target;
    use crate::resources::common::TlsConfig;

    let mut conn = ExternalHttpConnection {
        target: serde_yaml::from_str("url: HTTP://api.example.com").unwrap(),
        ..Default::default()
    };
    conn.tls = Some(TlsConfig {
        enabled: true,
        ..Default::default()
    });
    let err = validate_connection_target(&conn).expect("HTTP + tls.enabled must be rejected");
    assert!(err.contains("tls.enabled"), "got: {err}");
}

/// An https URL target with no tls block is valid (system CA bundle).
#[test]
fn test_https_url_without_tls_block_is_valid() {
    use crate::resources::common::external_http::validate_connection_target;

    let conn = ExternalHttpConnection {
        target: serde_yaml::from_str("url: https://api.example.com").unwrap(),
        ..Default::default()
    };
    assert_eq!(validate_connection_target(&conn), None);
}

/// Round-trip: an existing Service-shaped target re-serializes to the same flat YAML.
#[test]
fn test_service_target_roundtrip_is_byte_stable() {
    let yaml = "kind: Service\nname: auth-svc\nnamespace: prod\nport: 8080\n";
    let target: WebhookTarget = serde_yaml::from_str(yaml).unwrap();
    let out = serde_yaml::to_string(&target).unwrap();
    let reparsed: WebhookTarget = serde_yaml::from_str(&out).unwrap();
    assert_eq!(reparsed.service.name.as_deref(), Some("auth-svc"));
    assert_eq!(reparsed.service.namespace.as_deref(), Some("prod"));
    assert_eq!(reparsed.service.port, Some(8080));
    assert_eq!(reparsed.service.kind.as_deref(), Some("Service"));
    assert!(reparsed.url.is_none());
    assert!(
        !out.contains("url"),
        "a service target must not serialize a url key: {out}"
    );
    assert!(
        !out.contains("blockPrivate"),
        "a service target must not serialize blockPrivate: {out}"
    );
}

/// A URL target's ForwardAuth config validates.
#[test]
fn test_forward_auth_accepts_url_target() {
    use crate::resources::edgion_plugins::plugin_configs::ForwardAuthConfig;

    // `decision` has no serde default on `ForwardAuthConfig` (unrelated to Task 1),
    // so it must be present even though this test only exercises target validation.
    let cfg: ForwardAuthConfig = serde_yaml::from_str(
        "target:\n  url: https://auth.example.com\nrequest:\n  path:\n    template: /verify\ndecision: {}\n",
    )
    .unwrap();
    assert_eq!(cfg.validation_error(), None);
}

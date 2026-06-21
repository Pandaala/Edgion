//! Unit tests for `webhook.rs` (see `super`).
use super::*;

#[test]
fn test_default_config() {
    let config = WebhookServiceConfig::default();
    assert!(config.auth.is_none());
    assert!(config.request_method.is_none());
    assert_eq!(config.timeout_ms, 5000);
    assert_eq!(config.max_response_bytes, 1024);
    assert!(!config.allow_degradation);
    assert_eq!(config.status_on_error, 503);
    assert!(config.retry.is_none());
    assert!(config.rate_limit.is_none());
    assert!(config.health_check.is_none());
    // Default target has no port (will fail validation)
    assert!(config.target.backend_ref.port.is_none());
}

#[test]
fn test_validation_empty_default() {
    let config = WebhookServiceConfig::default();
    // Default has no port, so it fails validation
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("backendRef port is required")
    );
}

// URI variant removed; WebhookTarget now only supports Service backends

#[test]
fn test_validation_invalid_method_shim_field_not_validated() {
    // request_method is now a deprecated shim field and is not validated.
    // An invalid method value is accepted without error.
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "svc".to_string(),
                namespace: None,
                port: Some(80),
            },
        },
        request_method: Some(serde_json::json!("INVALID")),
        ..Default::default()
    };
    // Shim fields are not validated, so this should pass validation
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_validation_zero_timeout() {
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "svc".to_string(),
                namespace: None,
                port: Some(80),
            },
        },
        timeout_ms: 0,
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("timeoutMs must be greater than 0")
    );
}

#[test]
fn test_validation_valid_config() {
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "api-svc".to_string(),
                namespace: Some("prod".to_string()),
                port: Some(8080),
            },
        },
        ..Default::default()
    };
    assert!(config.get_validation_error().is_none());
}

// ---- Service target validation ----

#[test]
fn test_validation_service_no_port() {
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "my-svc".to_string(),
                namespace: None,
                port: None,
            },
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
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "my-svc".to_string(),
                namespace: None,
                port: Some(0),
            },
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("backendRef port must be > 0")
    );
}

#[test]
fn test_validation_service_port_negative() {
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "my-svc".to_string(),
                namespace: None,
                port: Some(-1),
            },
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("backendRef port must be > 0")
    );
}

#[test]
fn test_validation_service_valid() {
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "my-svc".to_string(),
                namespace: Some("prod".to_string()),
                port: Some(8080),
            },
        },
        ..Default::default()
    };
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_validation_service_kind_service_import() {
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: Some("ServiceImport".to_string()),
                name: "my-svc".to_string(),
                namespace: None,
                port: Some(80),
            },
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
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: Some("ServiceExternalName".to_string()),
                name: "my-svc".to_string(),
                namespace: None,
                port: Some(80),
            },
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
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: Some("BogusKind".to_string()),
                name: "my-svc".to_string(),
                namespace: None,
                port: Some(80),
            },
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
    assert_eq!(config.target.backend_ref.name, "api-svc");
    assert_eq!(config.target.backend_ref.port, Some(8080));
    assert!(config.request_method.is_none());
    assert_eq!(config.timeout_ms, 5000);
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
requestMethod: POST
timeoutMs: 3000
successStatusCodes: [200]
maxResponseBytes: 2048
allowDegradation: true
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
    assert_eq!(config.target.backend_ref.name, "key-service");
    assert_eq!(config.target.backend_ref.namespace, Some("internal".to_string()));
    assert_eq!(config.target.backend_ref.port, Some(443));
    // request_method is now a shim field; verify it deserialized as serde_json::Value
    assert!(config.request_method.is_some());
    assert_eq!(config.timeout_ms, 3000);
    assert_eq!(config.max_response_bytes, 2048);
    assert!(config.allow_degradation);
    // Validate before consuming fields with unwrap()
    assert!(config.get_validation_error().is_none());
    assert!(config.retry.is_some());
    let retry = config.retry.unwrap();
    assert_eq!(retry.max_retries, 1);
    assert!(retry.retry_on_timeout);
    assert_eq!(retry.retry_on_status, vec![502, 503, 504]);
    assert!(config.rate_limit.is_some());
    let rl = config.rate_limit.unwrap();
    assert_eq!(rl.rate, 200);
    assert_eq!(rl.window_sec, 1);
    assert!(config.health_check.is_some());
    let hc = config.health_check.unwrap();
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
    assert_eq!(config.target.backend_ref.name, "my-webhook-svc");
    assert_eq!(config.target.backend_ref.namespace, Some("prod".to_string()));
    assert_eq!(config.target.backend_ref.port, Some(8080));
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_serde_roundtrip() {
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: None,
                name: "example-svc".to_string(),
                namespace: None,
                port: Some(443),
            },
        },
        request_method: Some(serde_json::json!("POST")),
        ..Default::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: WebhookServiceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.target.backend_ref.name, config.target.backend_ref.name);
    assert_eq!(deserialized.target.backend_ref.port, config.target.backend_ref.port);
    assert_eq!(deserialized.request_method, config.request_method);
    assert_eq!(deserialized.timeout_ms, config.timeout_ms);
}

#[test]
fn test_serde_service_target_roundtrip() {
    // Verifies both serialization and deserialization of BackendObjectReference,
    // including optional fields.
    let config = WebhookServiceConfig {
        target: WebhookTarget {
            backend_ref: BackendObjectReference {
                group: String::new(),
                kind: Some("ServiceClusterIp".to_string()),
                name: "auth-svc".to_string(),
                namespace: Some("prod".to_string()),
                port: Some(8080),
            },
        },
        request_method: Some(serde_json::json!("POST")),
        ..Default::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    // Verify None/empty fields are omitted.
    assert!(!json.contains("\"group\""), "empty group must be omitted: {json}");

    let deserialized: WebhookServiceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.target.backend_ref.name, "auth-svc");
    assert_eq!(deserialized.target.backend_ref.namespace.as_deref(), Some("prod"));
    assert_eq!(deserialized.target.backend_ref.port, Some(8080));
    assert_eq!(
        deserialized.target.backend_ref.kind.as_deref(),
        Some("ServiceClusterIp")
    );
}

#[test]
fn service_target_serialization_has_deny_unknown_fields() {
    // Verify that deny_unknown_fields is enforced on WebhookTarget
    let json = r#"{"backendRef":{"name":"svc","port":80},"extraField":"should fail"}"#;
    let r: Result<WebhookTarget, _> = serde_json::from_str(json);
    assert!(r.is_err(), "deny_unknown_fields must reject extra fields: {:?}", r);
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

#[test]
fn test_webhook_auth_basic() {
    let yaml = r#"
type: basic
secretRef:
  name: webhook-creds
"#;
    let auth: WebhookAuth = serde_yaml::from_str(yaml).unwrap();
    match auth {
        WebhookAuth::Basic { slot } => {
            assert_eq!(slot.secret_ref.name, "webhook-creds");
        }
        _ => panic!("Expected Basic auth"),
    }
}

#[test]
fn test_webhook_auth_bearer() {
    let yaml = r#"
type: bearer
secretRef:
  name: webhook-token
  namespace: secrets
"#;
    let auth: WebhookAuth = serde_yaml::from_str(yaml).unwrap();
    match auth {
        WebhookAuth::Bearer { slot } => {
            assert_eq!(slot.secret_ref.name, "webhook-token");
            assert_eq!(slot.secret_ref.namespace.as_deref(), Some("secrets"));
        }
        _ => panic!("Expected Bearer auth"),
    }
}

#[test]
fn test_webhook_auth_hmac() {
    let yaml = r#"
type: hmac
secretRef:
  name: webhook-hmac-key
"#;
    let auth: WebhookAuth = serde_yaml::from_str(yaml).unwrap();
    match auth {
        WebhookAuth::Hmac { slot } => {
            assert_eq!(slot.secret_ref.name, "webhook-hmac-key");
        }
        _ => panic!("Expected Hmac auth"),
    }
}

#[test]
fn webhook_auth_rejects_inline_username() {
    let yaml = "type: basic\nsecretRef:\n  name: s\nusername: leaked";
    let r: Result<WebhookAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
}

#[test]
fn webhook_auth_rejects_inline_password() {
    let yaml = "type: basic\nsecretRef:\n  name: s\npassword: leaked";
    let r: Result<WebhookAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
}

#[test]
fn test_webhook_config_with_auth() {
    let yaml = r#"
target:
  name: "api-svc"
  port: 443
auth:
  type: bearer
  secretRef:
    name: api-token
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    // auth is now a shim field; verify it deserialized as serde_json::Value
    assert!(config.auth.is_some());
    let auth_value = config.auth.unwrap();
    // Verify the JSON structure contains the expected auth data
    assert!(auth_value.get("type").is_some());
    assert_eq!(auth_value["type"], "bearer");
}

#[test]
fn test_serde_default_headers() {
    let yaml = r#"
target:
  name: "api-svc"
  port: 80
defaultHeaders:
  X-Source: edgion
  Content-Type: "application/json"
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    // default_headers is now a shim field; verify it deserialized as serde_json::Value
    let headers_value = config.default_headers.unwrap();
    assert_eq!(headers_value["X-Source"], "edgion");
    assert_eq!(headers_value["Content-Type"], "application/json");
}

#[test]
fn test_webhook_auth_api_key_header() {
    let yaml = r#"
type: apiKey
secretRef:
  name: my-api-key
in: header
headerName: X-API-Key
"#;
    let auth: WebhookAuth = serde_yaml::from_str(yaml).unwrap();
    match auth {
        WebhookAuth::ApiKey {
            slot,
            r#in,
            header_name,
            ..
        } => {
            assert_eq!(slot.secret_ref.name, "my-api-key");
            assert_eq!(r#in, "header");
            assert_eq!(header_name.as_deref(), Some("X-API-Key"));
        }
        _ => panic!("Expected ApiKey auth"),
    }
}

#[test]
fn test_webhook_auth_api_key_query() {
    let yaml = r#"
type: apiKey
secretRef:
  name: my-api-key
in: query
queryParam: api_key
"#;
    let auth: WebhookAuth = serde_yaml::from_str(yaml).unwrap();
    match auth {
        WebhookAuth::ApiKey { r#in, query_param, .. } => {
            assert_eq!(r#in, "query");
            assert_eq!(query_param.as_deref(), Some("api_key"));
        }
        _ => panic!("Expected ApiKey auth"),
    }
}

#[test]
fn webhook_auth_basic_yaml_roundtrip() {
    let yaml = "type: basic\nsecretRef:\n  name: w-creds";
    let a: WebhookAuth = serde_yaml::from_str(yaml).unwrap();
    match a {
        WebhookAuth::Basic { slot } => assert_eq!(slot.secret_ref.name, "w-creds"),
        _ => panic!("expected Basic"),
    }
}

#[test]
fn webhook_auth_apikey_with_extras_yaml_roundtrip() {
    let yaml = "type: apiKey\nsecretRef:\n  name: w-creds\nin: header\nheaderName: X-API-Key";
    let a: WebhookAuth = serde_yaml::from_str(yaml).unwrap();
    match a {
        WebhookAuth::ApiKey {
            slot,
            r#in,
            header_name,
            query_param,
        } => {
            assert_eq!(slot.secret_ref.name, "w-creds");
            assert_eq!(r#in, "header");
            assert_eq!(header_name.as_deref(), Some("X-API-Key"));
            assert!(query_param.is_none());
        }
        _ => panic!("expected ApiKey"),
    }
}

#[test]
fn webhook_auth_basic_rejects_unknown_field_new() {
    let yaml = "type: basic\nsecretRef:\n  name: x\nbogus: 1";
    let r: Result<WebhookAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "expected error for unknown field, got {:?}", r);
}

// ============================================================
// WebhookTarget struct tests (Task A1)
// ============================================================

#[test]
fn test_webhook_target_valid_service_with_port() {
    // Valid: Service with required port
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: Some("Service".to_string()),
            name: "my-svc".to_string(),
            namespace: Some("prod".to_string()),
            port: Some(8080),
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "valid service should not have error: {:?}", error);
}

#[test]
fn test_webhook_target_missing_port_error() {
    // Invalid: port is None (required)
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: Some("Service".to_string()),
            name: "my-svc".to_string(),
            namespace: Some("prod".to_string()),
            port: None,
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_some(), "missing port should produce error");
    assert_eq!(error.unwrap(), "backendRef port is required");
}

#[test]
fn test_webhook_target_external_name_rejected() {
    // Invalid: ServiceExternalName kind is not allowed
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: Some("ServiceExternalName".to_string()),
            name: "external-svc".to_string(),
            namespace: None,
            port: Some(443),
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_some(), "ServiceExternalName should be rejected");
    assert_eq!(error.unwrap(), "ServiceExternalName is not supported for webhook");
}

#[test]
fn test_webhook_target_service_cluster_ip_valid() {
    // Valid: ServiceClusterIp is in allowed set
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: Some("ServiceClusterIp".to_string()),
            name: "cluster-svc".to_string(),
            namespace: Some("default".to_string()),
            port: Some(3000),
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "ServiceClusterIp should be valid: {:?}", error);
}

#[test]
fn test_webhook_target_service_endpoint_slice_valid() {
    // Valid: ServiceEndpointSlice is in allowed set
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: Some("ServiceEndpointSlice".to_string()),
            name: "endpoint-svc".to_string(),
            namespace: None,
            port: Some(9090),
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "ServiceEndpointSlice should be valid: {:?}", error);
}

#[test]
fn test_webhook_target_kind_empty_string_valid() {
    // Valid: empty kind defaults to Service
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: Some(String::new()),
            name: "my-svc".to_string(),
            namespace: None,
            port: Some(80),
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "empty kind should default to Service: {:?}", error);
}

#[test]
fn test_webhook_target_kind_none_valid() {
    // Valid: None kind defaults to Service
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: None,
            name: "my-svc".to_string(),
            namespace: None,
            port: Some(80),
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_none(), "None kind should default to Service: {:?}", error);
}

#[test]
fn test_webhook_target_unsupported_kind_rejected() {
    // Invalid: unsupported kind
    let target = WebhookTarget {
        backend_ref: BackendObjectReference {
            group: String::new(),
            kind: Some("UnsupportedKind".to_string()),
            name: "unknown-svc".to_string(),
            namespace: None,
            port: Some(80),
        },
    };
    let error = validate_webhook_target(&target);
    assert!(error.is_some(), "unsupported kind should be rejected");
    assert_eq!(error.unwrap(), "unsupported service kind: UnsupportedKind");
}

// ============================================================
// Shim field deserialization tests (Task A2)
// ============================================================

#[test]
fn test_shim_field_auth_with_bearer_deserializes() {
    // Old config with auth: { type: bearer, ... } should deserialize to serde_json::Value
    let yaml = r#"
target:
  name: "api-svc"
  port: 443
auth:
  type: bearer
  secretRef:
    name: api-token
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    // After shim field change, auth should be Some(serde_json::Value)
    assert!(config.auth.is_some(), "auth field should deserialize from old config");
}

#[test]
fn test_shim_field_default_headers_deserializes() {
    // Old config with defaultHeaders should deserialize to serde_json::Value
    let yaml = r#"
target:
  name: "api-svc"
  port: 80
defaultHeaders:
  X-Source: edgion
  Content-Type: "application/json"
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    // After shim field change, default_headers should be Some(serde_json::Value)
    assert!(
        config.default_headers.is_some(),
        "default_headers field should deserialize from old config"
    );
}

#[test]
fn test_shim_field_request_method_deserializes() {
    // Old config with requestMethod as string should deserialize to serde_json::Value
    let yaml = r#"
target:
  name: "api-svc"
  port: 80
requestMethod: POST
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    // After shim field change, request_method should be Some(serde_json::Value)
    assert!(
        config.request_method.is_some(),
        "request_method field should deserialize from old config"
    );
}

#[test]
fn test_shim_fields_absent_deserialize_to_none() {
    // New config without deprecated shim fields should deserialize with None values
    let yaml = r#"
target:
  name: "api-svc"
  port: 8080
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.auth.is_none(), "auth should be None when absent");
    assert!(
        config.default_headers.is_none(),
        "default_headers should be None when absent"
    );
    assert!(
        config.request_method.is_none(),
        "request_method should be None when absent"
    );
}

#[test]
fn test_deny_unknown_fields_rejects_typo() {
    // deny_unknown_fields should catch misspelled field names
    let yaml = r#"
target:
  name: "api-svc"
  port: 80
unknownFieldName: "should fail"
"#;
    let result: Result<WebhookServiceConfig, _> = serde_yaml::from_str(yaml);
    assert!(
        result.is_err(),
        "deny_unknown_fields must reject unknown fields: {:?}",
        result
    );
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
    assert!(config.target.backend_ref.port.is_some());
    assert_eq!(config.timeout_ms, 3000);
}

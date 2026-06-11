//! Unit tests for `webhook.rs` (see `super`).
use super::*;

#[test]
fn test_default_config() {
    let config = WebhookServiceConfig::default();
    assert!(config.auth.is_none());
    assert_eq!(config.request_method, "GET");
    assert_eq!(config.timeout_ms, 5000);
    assert_eq!(config.max_response_bytes, 1024);
    assert!(!config.allow_degradation);
    assert_eq!(config.status_on_error, 503);
    assert!(config.retry.is_none());
    assert!(config.rate_limit.is_none());
    assert!(config.health_check.is_none());
    // Default target is Uri with empty string
    matches!(config.target, WebhookTarget::Uri { ref uri } if uri.is_empty());
}

#[test]
fn test_validation_empty_uri() {
    let config = WebhookServiceConfig::default();
    assert_eq!(config.get_validation_error().as_deref(), Some("uri is required"));
}

#[test]
fn test_validation_invalid_uri() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Uri {
            uri: "ftp://invalid".into(),
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("uri must start with http:// or https://")
    );
}

#[test]
fn test_validation_invalid_method() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Uri {
            uri: "http://example.com/hook".into(),
        },
        request_method: "INVALID".to_string(),
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("requestMethod must be a valid HTTP method")
    );
}

#[test]
fn test_validation_zero_timeout() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Uri {
            uri: "http://example.com/hook".into(),
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
        target: WebhookTarget::Uri {
            uri: "https://api.example.com/resolve".into(),
        },
        ..Default::default()
    };
    assert!(config.get_validation_error().is_none());
}

// ---- Service target validation ----

#[test]
fn test_validation_service_empty_name() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Service {
            name: "".to_string(),
            namespace: None,
            port: None,
            group: None,
            kind: None,
            endpoints: vec![],
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("service name is required")
    );
}

#[test]
fn test_validation_service_port_zero() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Service {
            name: "my-svc".to_string(),
            namespace: None,
            port: Some(0),
            group: None,
            kind: None,
            endpoints: vec![],
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("service port must be > 0")
    );
}

#[test]
fn test_validation_service_port_negative() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Service {
            name: "my-svc".to_string(),
            namespace: None,
            port: Some(-1),
            group: None,
            kind: None,
            endpoints: vec![],
        },
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error().as_deref(),
        Some("service port must be > 0")
    );
}

#[test]
fn test_validation_service_valid() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Service {
            name: "my-svc".to_string(),
            namespace: Some("prod".to_string()),
            port: Some(8080),
            group: None,
            kind: None,
            endpoints: vec![],
        },
        ..Default::default()
    };
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_validation_service_kind_service_import() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Service {
            name: "my-svc".to_string(),
            namespace: None,
            port: Some(80),
            group: None,
            kind: Some("ServiceImport".to_string()),
            endpoints: vec![],
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
        target: WebhookTarget::Service {
            name: "my-svc".to_string(),
            namespace: None,
            port: Some(80),
            group: None,
            kind: Some("ServiceExternalName".to_string()),
            endpoints: vec![],
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
        target: WebhookTarget::Service {
            name: "my-svc".to_string(),
            namespace: None,
            port: Some(80),
            group: None,
            kind: Some("BogusKind".to_string()),
            endpoints: vec![],
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
  type: uri
  uri: "http://localhost:8080/resolve"
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    match &config.target {
        WebhookTarget::Uri { uri } => assert_eq!(uri, "http://localhost:8080/resolve"),
        _ => panic!("expected Uri target"),
    }
    assert_eq!(config.request_method, "GET");
    assert_eq!(config.timeout_ms, 5000);
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_serde_full_yaml() {
    let yaml = r#"
target:
  type: uri
  uri: "https://key-service.internal/resolve"
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
    match &config.target {
        WebhookTarget::Uri { uri } => {
            assert_eq!(uri, "https://key-service.internal/resolve")
        }
        _ => panic!("expected Uri target"),
    }
    assert_eq!(config.request_method, "POST");
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
  type: service
  name: my-webhook-svc
  namespace: prod
  port: 8080
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    match &config.target {
        WebhookTarget::Service {
            name, namespace, port, ..
        } => {
            assert_eq!(name, "my-webhook-svc");
            assert_eq!(namespace.as_deref(), Some("prod"));
            assert_eq!(*port, Some(8080));
        }
        _ => panic!("expected Service target"),
    }
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_serde_roundtrip() {
    let config = WebhookServiceConfig {
        target: WebhookTarget::Uri {
            uri: "https://example.com/api".into(),
        },
        request_method: "POST".to_string(),
        ..Default::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: WebhookServiceConfig = serde_json::from_str(&json).unwrap();
    match (&deserialized.target, &config.target) {
        (WebhookTarget::Uri { uri: u1 }, WebhookTarget::Uri { uri: u2 }) => {
            assert_eq!(u1, u2)
        }
        _ => panic!("target mismatch after roundtrip"),
    }
    assert_eq!(deserialized.request_method, config.request_method);
    assert_eq!(deserialized.timeout_ms, config.timeout_ms);
}

#[test]
fn test_serde_service_target_roundtrip() {
    // Verifies both serialization and deserialization of the Service variant,
    // including optional fields and endpoints injected by the Controller.
    let config = WebhookServiceConfig {
        target: WebhookTarget::Service {
            name: "auth-svc".into(),
            namespace: Some("prod".into()),
            port: Some(8080),
            group: None,
            kind: Some("ServiceClusterIp".into()),
            endpoints: vec!["10.0.0.1:8080".into(), "10.0.0.2:8080".into()],
        },
        request_method: "POST".into(),
        ..Default::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    // Verify "type":"service" tag is present in the output.
    assert!(
        json.contains("\"type\":\"service\""),
        "serialized JSON must contain type tag"
    );
    // Verify endpoints are serialized.
    assert!(json.contains("10.0.0.1:8080"), "endpoints must be serialized");
    // Verify None fields are omitted.
    assert!(!json.contains("\"group\""), "None group must be omitted");

    let deserialized: WebhookServiceConfig = serde_json::from_str(&json).unwrap();
    match deserialized.target {
        WebhookTarget::Service {
            name,
            namespace,
            port,
            kind,
            endpoints,
            ..
        } => {
            assert_eq!(name, "auth-svc");
            assert_eq!(namespace.as_deref(), Some("prod"));
            assert_eq!(port, Some(8080));
            assert_eq!(kind.as_deref(), Some("ServiceClusterIp"));
            assert_eq!(endpoints, vec!["10.0.0.1:8080", "10.0.0.2:8080"]);
        }
        _ => panic!("expected Service target after roundtrip"),
    }
}

#[test]
fn test_serde_service_target_empty_endpoints_omitted() {
    // endpoints: Vec::is_empty → skip_serializing_if ensures empty endpoints
    // do not appear in the serialized output (users never set them manually).
    let config = WebhookServiceConfig {
        target: WebhookTarget::Service {
            name: "svc".into(),
            namespace: None,
            port: Some(80),
            group: None,
            kind: None,
            endpoints: vec![],
        },
        ..Default::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        !json.contains("\"endpoints\""),
        "empty endpoints must be omitted from output"
    );
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
  type: uri
  uri: "https://api.example.com/resolve"
auth:
  type: bearer
  secretRef:
    name: api-token
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.auth.is_some());
    match config.auth.unwrap() {
        WebhookAuth::Bearer { slot } => {
            assert_eq!(slot.secret_ref.name, "api-token");
        }
        _ => panic!("Expected Bearer auth"),
    }
}

#[test]
fn test_serde_default_headers() {
    let yaml = r#"
target:
  type: uri
  uri: "http://api.example.com"
defaultHeaders:
  X-Source: edgion
  Content-Type: "application/json"
"#;
    let config: WebhookServiceConfig = serde_yaml::from_str(yaml).unwrap();
    let headers = config.default_headers.unwrap();
    assert_eq!(headers.get("X-Source").map(String::as_str), Some("edgion"));
    assert_eq!(
        headers.get("Content-Type").map(String::as_str),
        Some("application/json")
    );
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

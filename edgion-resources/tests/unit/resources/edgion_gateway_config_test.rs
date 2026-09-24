use super::*;

#[test]
fn server_keepalive_request_limit_defaults_to_ten_thousand() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "server": {}
    }))
    .unwrap();

    assert_eq!(spec.server.unwrap().downstream_keepalive_request_limit, 10_000);
}

#[test]
fn preflight_policy_accepts_only_ordinary_final_statuses() {
    let valid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "preflightPolicy": {"statusCode": 204}
    }))
    .unwrap();
    assert!(valid.validate_preflight_policy().is_empty());

    for status in [100, 101, 103, 600, 700] {
        let invalid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "preflightPolicy": {"statusCode": status}
        }))
        .unwrap();
        assert_eq!(invalid.validate_preflight_policy().len(), 1, "{status}");
    }
}

#[test]
fn forwarded_headers_default_to_x_real_ip() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
    assert_eq!(spec.forwarded_headers.remote_ip_header, "X-Real-IP");
    assert!(spec.validate_forwarded_headers().is_empty());
}

#[test]
fn forwarded_headers_accept_custom_remote_ip_header() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "forwardedHeaders": {"remoteIpHeader": "X-Edgion-Remote-IP"}
    }))
    .unwrap();
    assert_eq!(spec.forwarded_headers.remote_ip_header, "X-Edgion-Remote-IP");
    assert!(spec.validate_forwarded_headers().is_empty());
}

#[test]
fn forwarded_headers_reject_invalid_and_reserved_remote_ip_headers() {
    for name in [
        "",
        "bad header",
        "Host",
        "Content-Length",
        "Connection",
        "X-Forwarded-For",
    ] {
        let spec: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "forwardedHeaders": {"remoteIpHeader": name}
        }))
        .unwrap();
        assert_eq!(spec.validate_forwarded_headers().len(), 1, "{name} must be rejected");
    }
}

#[test]
fn egc_outbound_tls_roundtrips() {
    let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "outboundTls": {"verify": false}
    }))
    .unwrap();
    assert!(!s.outbound_tls.unwrap().verify);
}

#[test]
fn request_body_defaults_are_gateway_class_policy() {
    let s: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
    assert!(s.request_body.enabled);
    assert_eq!(s.request_body.default_memory_buffer_size, "128KiB");
    assert_eq!(s.request_body.default_max_body_size, "32MiB");
    assert_eq!(s.request_body.storage_operation_timeout, "5s");
    assert!(s.validate_request_body().is_empty());
}

#[test]
fn request_body_overrides_parse_and_validate() {
    let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "requestBody": {
            "maxMemoryBufferSize": "256KiB",
            "maxBodySize": "8MiB",
            "storageOperationTimeout": "750ms"
        }
    }))
    .unwrap();
    let request_body = s.request_body;
    assert_eq!(request_body.max_memory_buffer_size.as_deref(), Some("256KiB"));
    assert_eq!(request_body.max_body_size.as_deref(), Some("8MiB"));
    assert_eq!(request_body.storage_operation_timeout, "750ms");
    assert!(request_body.validate().is_empty());
}

#[test]
fn request_body_rejects_invalid_and_inverted_limits() {
    let invalid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "requestBody": {
            "maxMemoryBufferSize": "garbage",
            "maxBodySize": "0"
        }
    }))
    .unwrap();
    assert_eq!(invalid.validate_request_body().len(), 2);

    let invalid_override: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "requestBody": {
            "maxMemoryBufferSize": "garbage",
            "maxBodySize": "1"
        }
    }))
    .unwrap();
    assert_eq!(invalid_override.validate_request_body().len(), 1);

    let inverted: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "requestBody": {
            "maxMemoryBufferSize": "2MiB",
            "maxBodySize": "1MiB"
        }
    }))
    .unwrap();
    assert_eq!(
        inverted.validate_request_body(),
        vec!["effective requestBody memory buffer size must not exceed the effective requestBody max body size"]
    );

    for timeout in ["0s", "invalid"] {
        let invalid_timeout: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "requestBody": { "storageOperationTimeout": timeout }
        }))
        .unwrap();
        assert_eq!(invalid_timeout.validate_request_body().len(), 1, "{timeout}");
    }
}

#[test]
fn egc_dns_resolver_absent_is_none() {
    let s: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
    assert!(s.dns_resolver.is_none());
    assert!(s.validate_dns_resolver().is_empty());
}

#[test]
fn egc_dns_resolver_link_sys_ref_roundtrips() {
    let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "dnsResolver": {"linkSysRef": {"namespace": "dns", "name": "aliyun"}}
    }))
    .unwrap();
    let dr = s.dns_resolver.as_ref().unwrap();
    let r = dr.link_sys_ref.as_ref().unwrap();
    assert_eq!(r.namespace, "dns");
    assert_eq!(r.name, "aliyun");
    // cacheTtl defaults to 5s when omitted.
    assert_eq!(dr.cache_ttl, "5s");
    assert!(s.validate_dns_resolver().is_empty());
}

#[test]
fn egc_dns_resolver_servers_roundtrip_and_cache_ttl() {
    let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "dnsResolver": {"servers": ["1.1.1.1", "8.8.8.8:5353"], "cacheTtl": "10s"}
    }))
    .unwrap();
    let dr = s.dns_resolver.as_ref().unwrap();
    assert_eq!(dr.servers.as_ref().unwrap().len(), 2);
    assert_eq!(dr.cache_ttl, "10s");
    assert!(s.validate_dns_resolver().is_empty());
}

#[test]
fn egc_dns_resolver_ref_and_servers_are_mutually_exclusive() {
    let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "dnsResolver": {
            "linkSysRef": {"namespace": "dns", "name": "aliyun"},
            "servers": ["1.1.1.1"]
        }
    }))
    .unwrap();
    let errs = s.validate_dns_resolver();
    assert!(
        errs.iter().any(|e| e.contains("mutually exclusive")),
        "both linkSysRef and servers set must be rejected, got: {errs:?}"
    );
}

#[test]
fn link_sys_max_instances_defaults_and_validates() {
    let omitted: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "linkSys": {"webhookMaxResponseBytes": 16384}
    }))
    .unwrap();
    assert_eq!(
        omitted.link_sys.as_ref().unwrap().max_instances_per_kind,
        LINK_SYS_DEFAULT_MAX_INSTANCES_PER_KIND
    );
    assert!(omitted.validate_link_sys().is_empty());

    let valid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "linkSys": {"maxInstancesPerKind": 50}
    }))
    .unwrap();
    assert_eq!(valid.link_sys.as_ref().unwrap().max_instances_per_kind, 50);
    assert!(valid.validate_link_sys().is_empty());

    for value in [0_usize, LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT + 1] {
        let invalid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
            "linkSys": {"maxInstancesPerKind": value}
        }))
        .unwrap();
        let errs = invalid.validate_link_sys();
        assert_eq!(errs.len(), 1, "{value} must be rejected, got: {errs:?}");
        assert!(errs[0].contains("maxInstancesPerKind"), "{errs:?}");
    }
}

#[test]
fn test_deserialize_gateway_config() {
    let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-config
spec:
  httpTimeout:
    client:
      readTimeout: 60s
      writeTimeout: 60s
    backend:
      defaultConnectTimeout: 5s
      defaultRequestTimeout: 60s
"#;
    let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Failed to parse YAML");
    assert_eq!(config.metadata.name, Some("test-config".to_string()));

    let spec = config.spec;
    assert!(spec.http_timeout.is_some());
}

#[test]
fn retry_defaults_to_two_attempts() {
    let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-default
spec: {}
"#;
    let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Should parse defaults");
    assert_eq!(config.spec.retry.attempts, 2);
}

#[test]
fn retry_attempts_parse_and_validate_at_domain_boundary() {
    let yaml = r#"
apiVersion: example.com/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: test-retry
spec:
  retry:
    attempts: 10
"#;
    let config: EdgionGatewayConfig = serde_yaml::from_str(yaml).expect("Should parse");
    assert_eq!(config.spec.retry.attempts, 10);
    assert!(config.spec.validate_retry().is_empty());

    let mut spec = EdgionGatewayConfigSpec::default();
    spec.retry.attempts = MAX_RETRY_ATTEMPTS;
    assert!(spec.validate_retry().is_empty());

    spec.retry.attempts = MAX_RETRY_ATTEMPTS + 1;
    let errors = spec.validate_retry();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("retry.attempts"));
}

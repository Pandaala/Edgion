use super::*;

#[test]
fn generic_requires_url_template() {
    let cfg = HttpDnsResolverConfig {
        preset: None,
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::System,
        connection: None,
    };
    let errs = cfg.validate();
    assert!(
        errs.iter().any(|e| e.contains("urlTemplate")),
        "generic (no preset) must require urlTemplate, got {errs:?}"
    );
}

#[test]
fn preset_is_valid_without_url_template() {
    let cfg = HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::System,
        connection: None,
    };
    assert!(cfg.validate().is_empty(), "preset config must validate clean");
}

#[test]
fn dns_fallback_requires_servers() {
    let cfg = HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::Dns { servers: vec![] },
        connection: None,
    };
    assert!(cfg.validate().iter().any(|e| e.contains("fallback")));
}

#[test]
fn dns_fallback_rejects_invalid_server_string() {
    let cfg = HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::Dns {
            servers: vec!["not-an-ip".to_string()],
        },
        connection: None,
    };
    let errs = cfg.validate();
    assert!(
        errs.iter().any(|e| e.contains("not-an-ip")),
        "expected an error naming the invalid server, got {errs:?}"
    );
}

#[test]
fn http_dns_connection_roundtrips_without_target() {
    // The lean HttpDnsConnection must parse with only tls/timeout and NO
    // `target` field (unlike the webhook ExternalHttpConnection it replaced).
    let yaml = "timeoutMs: 1500\n";
    let conn: HttpDnsConnection = yaml_serde::from_str(yaml).expect("parse lean connection");
    assert_eq!(conn.timeout_ms, Some(1500));
    assert!(conn.tls.is_none());
    assert!(!conn.allow_loopback);

    // Round-trip: serialized form must not contain a `target` key.
    let out = yaml_serde::to_string(&conn).unwrap();
    assert!(
        !out.contains("target"),
        "lean connection must not carry a target, got: {out}"
    );
}

#[test]
fn loopback_policy_is_typed_schema_visible_and_survives_linksys_transport() {
    use crate::resources::link_sys::LinkSys;
    for allow in [false, true] {
        let input = serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "LinkSys",
            "metadata": {"name": "dns", "namespace": "default"},
            "spec": {"type": "httpdns", "config": {
                "preset": "aliyun",
                "connection": {"allowLoopback": allow}
            }}
        });
        let link: LinkSys = serde_json::from_value(input).unwrap();
        let config = link.spec.config.as_http_dns().unwrap();
        assert_eq!(config.connection.as_ref().unwrap().allow_loopback, allow);
        assert!(config.validate().is_empty());
        let serialized = serde_json::to_value(&link).unwrap();
        assert_eq!(serialized["spec"]["config"]["connection"]["allowLoopback"], allow);
        let restored: LinkSys = serde_json::from_value(serialized).unwrap();
        assert_eq!(
            restored
                .spec
                .config
                .as_http_dns()
                .unwrap()
                .connection
                .as_ref()
                .unwrap()
                .allow_loopback,
            allow
        );
    }
    assert!(serde_json::from_value::<HttpDnsConnection>(serde_json::json!({"allowLoopback": "true"})).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(HttpDnsConnection)).unwrap();
    assert_eq!(schema["properties"]["allowLoopback"]["type"], "boolean");
    assert_eq!(schema["properties"]["allowLoopback"]["default"], false);
}

#[test]
fn http_dns_config_with_lean_connection_roundtrips() {
    let cfg = HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::System,
        connection: Some(HttpDnsConnection {
            allow_loopback: false,
            tls: None,
            timeout_ms: Some(2000),
            ..Default::default()
        }),
    };
    let out = yaml_serde::to_string(&cfg).unwrap();
    let back: HttpDnsResolverConfig = yaml_serde::from_str(&out).unwrap();
    assert_eq!(back.connection.and_then(|c| c.timeout_ms), Some(2000));
}

#[test]
fn preset_url_templates_contain_domain_placeholder() {
    assert!(HttpDnsPreset::Aliyun.url_template().contains("{domain}"));
    assert!(HttpDnsPreset::Tencent.url_template().contains("{domain}"));
}

#[test]
fn preset_response_formats_match_vendor_shapes() {
    let aliyun = HttpDnsPreset::Aliyun.response_format();
    assert_eq!(aliyun.kind, HttpDnsBodyKind::Json);
    assert_eq!(aliyun.ip_path.as_deref(), Some("ips"));

    let tencent = HttpDnsPreset::Tencent.response_format();
    assert_eq!(tencent.kind, HttpDnsBodyKind::Delimited);
    assert_eq!(tencent.delimiter.as_deref(), Some(";"));
}

#[test]
fn dns_fallback_accepts_valid_server_strings() {
    let cfg = HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::Dns {
            servers: vec!["1.1.1.1".to_string(), "8.8.8.8:5353".to_string(), "[::1]".to_string()],
        },
        connection: None,
    };
    assert!(cfg.validate().is_empty(), "valid servers must validate clean");
}

#[test]
fn connection_bounds_defaults_and_typed_roundtrip() {
    let defaults: HttpDnsConnection = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(defaults.validate().is_empty());
    assert_eq!(defaults.timeout_ms, None);
    assert_eq!(HttpDnsConnection::DEFAULT_TIMEOUT_MS, 3000);
    assert_eq!(HttpDnsConnection::DEFAULT_POOL_MAX_IDLE_PER_HOST, 8);
    assert_eq!(HttpDnsConnection::DEFAULT_POOL_IDLE_TIMEOUT_MS, 90000);
    assert_eq!(HttpDnsConnection::MAX_RESPONSE_BYTES, 65536);
    for (field, valid, invalid) in [
        ("timeoutMs", vec![1, 120000], vec![0, 120001]),
        ("connectTimeoutMs", vec![1, 3000], vec![0, 3001, 120001]),
        ("poolMaxIdlePerHost", vec![0, 256], vec![257]),
        ("poolIdleTimeoutMs", vec![1, 600000], vec![0, 600001]),
        ("maxResponseBytes", vec![1, 65536], vec![0, 65537]),
    ] {
        for value in valid {
            let input = serde_json::json!({field: value});
            let connection: HttpDnsConnection = serde_json::from_value(input.clone()).unwrap();
            assert!(connection.validate().is_empty(), "{input}: {:?}", connection.validate());
            assert_eq!(serde_json::to_value(&connection).unwrap()[field], value);
        }
        for value in invalid {
            let connection: HttpDnsConnection = serde_json::from_value(serde_json::json!({field: value})).unwrap();
            assert!(!connection.validate().is_empty(), "{field}={value} must be rejected");
        }
        assert!(serde_json::from_value::<HttpDnsConnection>(serde_json::json!({field: "1"})).is_err());
        let schema = serde_json::to_value(schemars::schema_for!(HttpDnsConnection)).unwrap();
        assert!(schema["properties"].get(field).is_some());
    }
    let connection: HttpDnsConnection =
        serde_json::from_value(serde_json::json!({"timeoutMs": 50, "connectTimeoutMs": 51})).unwrap();
    assert!(!connection.validate().is_empty());
    let config: HttpDnsResolverConfig =
        serde_json::from_value(serde_json::json!({"preset": "aliyun", "connection": {"maxResponseBytes": 65537}}))
            .unwrap();
    assert!(
        !config.validate().is_empty(),
        "resource validation must include transport bounds"
    );
}

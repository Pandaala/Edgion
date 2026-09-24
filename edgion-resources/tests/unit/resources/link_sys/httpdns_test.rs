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
    let conn: HttpDnsConnection = serde_yaml::from_str(yaml).expect("parse lean connection");
    assert_eq!(conn.timeout_ms, Some(1500));
    assert!(conn.tls.is_none());

    // Round-trip: serialized form must not contain a `target` key.
    let out = serde_yaml::to_string(&conn).unwrap();
    assert!(
        !out.contains("target"),
        "lean connection must not carry a target, got: {out}"
    );
}

#[test]
fn http_dns_config_with_lean_connection_roundtrips() {
    let cfg = HttpDnsResolverConfig {
        preset: Some(HttpDnsPreset::Aliyun),
        url_template: None,
        response: HttpDnsResponseFormat::default(),
        fallback: HttpDnsFallback::System,
        connection: Some(HttpDnsConnection {
            tls: None,
            timeout_ms: Some(2000),
        }),
    };
    let out = serde_yaml::to_string(&cfg).unwrap();
    let back: HttpDnsResolverConfig = serde_yaml::from_str(&out).unwrap();
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

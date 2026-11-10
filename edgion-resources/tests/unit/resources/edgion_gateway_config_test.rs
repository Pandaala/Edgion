use super::*;

#[test]
fn dns_lookup_family_defaults_strictly_and_round_trips() {
    let default: DnsResolverConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(default.lookup_family, DnsLookupFamily::Ipv4Only);
    for (token, family) in [
        ("ipv4Only", DnsLookupFamily::Ipv4Only),
        ("ipv6Only", DnsLookupFamily::Ipv6Only),
        ("all", DnsLookupFamily::All),
    ] {
        let config: DnsResolverConfig = serde_json::from_value(serde_json::json!({"lookupFamily":token})).unwrap();
        assert_eq!(config.lookup_family, family);
        assert_eq!(serde_json::to_value(&config).unwrap()["lookupFamily"], token);
    }
    for token in ["", "ipv4", "ALL", "auto"] {
        assert!(serde_json::from_value::<DnsResolverConfig>(serde_json::json!({"lookupFamily":token})).is_err());
    }
}

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
    assert_eq!(s.request_body.storage.memory_limit, "128KiB");
    assert!(s.request_body.storage.spill_to_disk);
    assert!(s.request_body.storage.max_buffered_size.is_none());
    assert_eq!(s.request_body.max_body_size, "32MiB");
    assert_eq!(s.request_body.storage.operation_timeout, "5s");
    assert!(s.validate_request_body().is_empty());
}

#[test]
fn request_body_overrides_parse_and_validate() {
    let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "requestBody": {"maxBodySize": "8MiB", "storage": {
            "memoryLimit": "256KiB", "spillToDisk": false,
            "maxBufferedSize": "2MiB", "operationTimeout": "750ms"
        }}
    }))
    .unwrap();
    assert_eq!(s.request_body.storage.memory_limit, "256KiB");
    assert_eq!(s.request_body.max_body_size, "8MiB");
    assert_eq!(s.request_body.storage.operation_timeout, "750ms");
    assert!(!s.request_body.storage.spill_to_disk);
    assert!(s.validate_request_body().is_empty());
}

#[test]
fn request_body_rejects_invalid_and_inverted_limits() {
    for body in [
        serde_json::json!({"maxBodySize": "0"}),
        serde_json::json!({"storage": {"memoryLimit": "garbage"}}),
        serde_json::json!({"storage": {"memoryLimit": "0"}}),
        serde_json::json!({"storage": {"maxBufferedSize": "0"}}),
        serde_json::json!({"storage": {"maxBufferedSize": "garbage"}}),
        serde_json::json!({"maxBodySize": "1MiB", "storage": {"memoryLimit": "2MiB"}}),
        serde_json::json!({"storage": {"maxBufferedSize": "64KiB"}}),
        serde_json::json!({"storage": {"maxBufferedSize": "64MiB"}}),
        serde_json::json!({"storage": {"operationTimeout": "0s"}}),
        serde_json::json!({"storage": {"operationTimeout": "invalid"}}),
    ] {
        let config: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({"requestBody": body})).unwrap();
        assert!(!config.validate_request_body().is_empty(), "{body}");
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
    assert!(s.validate_dns_resolver().is_empty());
}

#[test]
fn egc_dns_resolver_refresh_defaults() {
    let dr: DnsResolverConfig = serde_json::from_str("{}").unwrap();
    assert!(dr.endpoint_table);
    assert_eq!(dr.ttl_factor, 0.5);
    assert_eq!(dr.min_refresh_seconds, 5);
    assert_eq!(dr.max_refresh_seconds, 300);
    assert_eq!(dr.fallback_refresh_seconds, None);
    assert_eq!(dr.refresh_interval_seconds, None);
    assert_eq!(dr.max_stale_seconds, 300);
    assert_eq!(dr.max_entries, 4096);
    assert!(dr.refresh_errors().is_empty());
    let default = DnsResolverConfig::default();
    assert_eq!(
        serde_json::to_value(&default).unwrap(),
        serde_json::to_value(&dr).unwrap()
    );
}

#[test]
fn egc_dns_resolver_servers_roundtrip_and_refresh_fields() {
    let s: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "dnsResolver": {
            "servers": ["1.1.1.1", "8.8.8.8:5353"],
            "endpointTable": false,
            "ttlFactor": 0.25,
            "minRefreshSeconds": 2,
            "maxRefreshSeconds": 2147483647,
            "refreshIntervalSeconds": 30
        }
    }))
    .unwrap();
    let dr = s.dns_resolver.as_ref().unwrap();
    assert_eq!(dr.servers.as_ref().unwrap().len(), 2);
    assert!(!dr.endpoint_table);
    assert_eq!(dr.ttl_factor, 0.25);
    assert_eq!(dr.min_refresh_seconds, 2);
    assert_eq!(dr.max_refresh_seconds, DNS_MAX_TTL_SECONDS);
    assert_eq!(dr.refresh_interval_seconds, Some(30));
    assert!(s.validate_dns_resolver().is_empty());
}

#[test]
fn egc_dns_resolver_rejects_out_of_range_refresh_settings() {
    let cases = [
        (serde_json::json!({"minRefreshSeconds": 1}), "minRefreshSeconds"),
        (
            serde_json::json!({"minRefreshSeconds": 60, "maxRefreshSeconds": 30, "fallbackRefreshSeconds": 30}),
            "must not exceed",
        ),
        (
            serde_json::json!({"fallbackRefreshSeconds": 600}),
            "fallbackRefreshSeconds",
        ),
        (
            serde_json::json!({"refreshIntervalSeconds": 1}),
            "refreshIntervalSeconds",
        ),
        (serde_json::json!({"ttlFactor": 0.0}), "ttlFactor"),
        (serde_json::json!({"ttlFactor": 0.005}), "ttlFactor"),
        (serde_json::json!({"ttlFactor": 1.5}), "ttlFactor"),
        (serde_json::json!({"maxEntries": 0}), "maxEntries"),
        (serde_json::json!({"maxEntries": 65537}), "maxEntries"),
        (serde_json::json!({"maxStaleSeconds": 2147483648u64}), "maxStaleSeconds"),
    ];
    for (config, needle) in cases {
        let dr: DnsResolverConfig = serde_json::from_value(config.clone()).unwrap();
        let errs = dr.refresh_errors();
        assert!(
            errs.iter().any(|error| error.contains(needle)),
            "{config} must report {needle}: {errs:?}"
        );
    }
}

#[test]
fn egc_dns_resolver_rejects_unparsable_servers() {
    let valid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "dnsResolver": {"servers": ["1.1.1.1", "8.8.8.8:5353", "[2001:db8::53]", "[2001:db8::53]:5353", "2001:db8::53"]}
    }))
    .unwrap();
    assert!(valid.validate_dns_resolver().is_empty());

    let invalid: EdgionGatewayConfigSpec = serde_json::from_value(serde_json::json!({
        "dnsResolver": {"servers": ["1.1.1.1", "10.0.0.1,10.0.0.2", "dns.example", ""]}
    }))
    .unwrap();
    let errs = invalid.validate_dns_resolver();
    assert_eq!(errs.len(), 3, "each unparsable entry is reported: {errs:?}");
    assert!(errs[0].contains("10.0.0.1,10.0.0.2"));
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
    let config: EdgionGatewayConfig = yaml_serde::from_str(yaml).expect("Failed to parse YAML");
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
    let config: EdgionGatewayConfig = yaml_serde::from_str(yaml).expect("Should parse defaults");
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
    let config: EdgionGatewayConfig = yaml_serde::from_str(yaml).expect("Should parse");
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

#[test]
fn downstream_tls_curves_round_trip_without_implicit_default() {
    let empty: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
    assert!(empty.downstream_tls.is_none());
    assert!(serde_json::to_value(empty).unwrap().get("downstreamTls").is_none());
    let spec: EdgionGatewayConfigSpec =
        serde_json::from_value(serde_json::json!({"downstreamTls":{"ecdhCurves":["P-384","X25519"]}})).unwrap();
    assert_eq!(
        serde_json::to_value(spec).unwrap()["downstreamTls"]["ecdhCurves"],
        serde_json::json!(["P-384", "X25519"])
    );
}

#[test]
fn egc_dns_resolver_single_bound_edits_need_no_fallback() {
    // Moving the range away from the default 5 s fallback never requires an
    // explicit fallbackRefreshSeconds: the unset fallback is clamped into range.
    for config in [
        serde_json::json!({"minRefreshSeconds": 10}),
        serde_json::json!({"minRefreshSeconds": 2, "maxRefreshSeconds": 3}),
        serde_json::json!({"minRefreshSeconds": 2, "maxRefreshSeconds": 4}),
    ] {
        let dr: DnsResolverConfig = serde_json::from_value(config.clone()).unwrap();
        assert!(dr.refresh_errors().is_empty(), "{config}: {:?}", dr.refresh_errors());
    }
    // A max below the default min is an inverted range, not a fallback problem.
    let dr: DnsResolverConfig = serde_json::from_value(serde_json::json!({"maxRefreshSeconds": 3})).unwrap();
    let errors = dr.refresh_errors();
    assert!(
        errors.iter().any(|e| e.contains("must not exceed maxRefreshSeconds")),
        "{errors:?}"
    );
    assert!(
        !errors.iter().any(|e| e.contains("fallbackRefreshSeconds")),
        "{errors:?}"
    );
    let dr: DnsResolverConfig = serde_json::from_value(serde_json::json!({"fallbackRefreshSeconds": 3})).unwrap();
    assert!(
        dr.refresh_errors().iter().any(|e| e.contains("fallbackRefreshSeconds")),
        "an explicit fallback below min is still rejected"
    );
}

#[test]
fn egc_dns_resolver_rejects_non_finite_ttl_factor() {
    let yaml = "ttlFactor: .nan\n";
    assert!(yaml_serde::from_str::<DnsResolverConfig>(yaml).is_err());
    let yaml = "ttlFactor: .inf\n";
    assert!(yaml_serde::from_str::<DnsResolverConfig>(yaml).is_err());
    assert!(yaml_serde::from_str::<DnsResolverConfig>("ttlFactor: 0.25\n").is_ok());
}

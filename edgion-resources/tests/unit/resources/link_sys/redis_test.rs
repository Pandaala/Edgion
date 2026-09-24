use super::*;

fn minimal_config() -> RedisClientConfig {
    RedisClientConfig {
        endpoints: vec!["redis://redis.example.com:6379".to_string()],
        auth: None,
        db: None,
        timeout: None,
        pool: None,
        topology: None,
        tls: None,
    }
}

#[test]
fn redis_configuration_contract_accepts_supported_boundaries() {
    let config = RedisClientConfig {
        db: Some(255),
        timeout: Some(RedisTimeout {
            connect: Some("10s".to_string()),
            command: Some("30s".to_string()),
        }),
        pool: Some(RedisPool {
            size: Some(REDIS_MAX_POOL_SIZE),
        }),
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Cluster,
            sentinel: None,
            cluster: Some(RedisCluster {
                max_redirects: Some(REDIS_MAX_REDIRECTS),
            }),
        }),
        ..minimal_config()
    };

    assert!(config.validation_errors().is_empty());
    assert_eq!(config.connect_timeout().unwrap(), REDIS_MAX_CONNECT_TIMEOUT);
    assert_eq!(config.command_timeout().unwrap(), REDIS_MAX_COMMAND_TIMEOUT);
}

#[test]
fn redis_configuration_contract_rejects_unsupported_values() {
    let config = RedisClientConfig {
        db: Some(256),
        timeout: Some(RedisTimeout {
            connect: Some("0s".to_string()),
            command: Some("31s".to_string()),
        }),
        pool: Some(RedisPool { size: Some(0) }),
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Cluster,
            sentinel: None,
            cluster: Some(RedisCluster {
                max_redirects: Some(REDIS_MAX_REDIRECTS + 1),
            }),
        }),
        ..minimal_config()
    };
    let errors = config.validation_errors().join("; ");

    assert!(errors.contains("db must be between 0 and 255"));
    assert!(errors.contains("pool.size must be between 1 and 64"));
    assert!(errors.contains("timeout.connect must be greater than zero"));
    assert!(errors.contains("timeout.command must not exceed 30000ms"));
    assert!(errors.contains("maxRedirects must not exceed 64"));
}

#[test]
fn redis_endpoint_contract_accepts_supported_authority_forms() {
    for (raw, host, port, transport) in [
        ("cache", "cache", 6379, RedisTransport::Plaintext),
        ("cache:6380", "cache", 6380, RedisTransport::Plaintext),
        ("redis://cache:6381", "cache", 6381, RedisTransport::Plaintext),
        ("rediss://cache", "cache", 6379, RedisTransport::Tls),
        ("[2001:db8::1]:6382", "2001:db8::1", 6382, RedisTransport::Plaintext),
    ] {
        let config = RedisClientConfig {
            endpoints: vec![raw.to_string()],
            ..minimal_config()
        };
        let parsed = config.parsed_active_endpoints().unwrap();
        assert_eq!(
            parsed,
            vec![ParsedRedisEndpoint {
                host: host.to_string(),
                port,
                transport,
            }]
        );
    }
}

#[test]
fn redis_endpoint_contract_rejects_unsupported_components_without_echoing_values() {
    for raw in [
        "",
        " redis://cache:6379",
        "http://cache:6379",
        "redis://user:secret@cache:6379",
        "redis://cache:6379/0",
        "redis://cache/%2e%2e",
        "redis://cache:6379?db=0",
        "redis://cache:6379#fragment",
        "redis://cache:0",
        "redis://cache:",
        "cache:",
    ] {
        let config = RedisClientConfig {
            endpoints: vec![raw.to_string()],
            ..minimal_config()
        };
        let errors = config.validation_errors().join("; ");
        assert!(errors.contains("Redis endpoints[0] is invalid"), "{raw}: {errors}");
        if !raw.is_empty() {
            assert!(
                !errors.contains(raw),
                "validation must not echo endpoint input: {errors}"
            );
        }
        assert!(
            !errors.contains("secret"),
            "validation must not echo accidental credentials: {errors}"
        );
    }
}

#[test]
fn redis_endpoint_contract_enforces_topology_cardinality_and_active_fields() {
    let standalone = RedisClientConfig {
        endpoints: vec!["redis://one:6379".into(), "redis://two:6379".into()],
        ..minimal_config()
    };
    assert!(standalone
        .validation_errors()
        .join("; ")
        .contains("standalone mode requires exactly one endpoint"));

    let cluster = RedisClientConfig {
        endpoints: Vec::new(),
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Cluster,
            sentinel: None,
            cluster: None,
        }),
        ..minimal_config()
    };
    assert!(cluster
        .validation_errors()
        .join("; ")
        .contains("cluster mode requires at least one endpoint"));

    let sentinel = RedisClientConfig {
        endpoints: Vec::new(),
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Sentinel,
            sentinel: Some(RedisSentinel {
                master_name: "mymaster".into(),
                sentinels: vec!["redis://sentinel-1:26379".into(), "sentinel-2:26379".into()],
            }),
            cluster: None,
        }),
        ..minimal_config()
    };
    assert_eq!(sentinel.parsed_active_endpoints().unwrap().len(), 2);

    let sentinel_with_unused_top_level = RedisClientConfig {
        endpoints: vec!["redis://unused:6379".into()],
        ..sentinel
    };
    assert!(sentinel_with_unused_top_level
        .validation_errors()
        .join("; ")
        .contains("sentinel mode must not configure top-level endpoints"));

    let invalid_sentinel = RedisClientConfig {
        endpoints: Vec::new(),
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Sentinel,
            sentinel: Some(RedisSentinel {
                master_name: " ".into(),
                sentinels: Vec::new(),
            }),
            cluster: Some(RedisCluster { max_redirects: None }),
        }),
        ..minimal_config()
    };
    let errors = invalid_sentinel.validation_errors().join("; ");
    assert!(errors.contains("must not configure topology.cluster"));
    assert!(errors.contains("masterName must not be empty"));
    assert!(errors.contains("requires at least one topology.sentinel.sentinels endpoint"));

    let standalone_with_inactive_config = RedisClientConfig {
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Standalone,
            sentinel: None,
            cluster: Some(RedisCluster { max_redirects: None }),
        }),
        ..minimal_config()
    };
    assert!(standalone_with_inactive_config
        .validation_errors()
        .join("; ")
        .contains("standalone mode must not configure topology.cluster"));
}

#[test]
fn redis_endpoint_contract_enforces_one_active_transport_without_explicit_tls() {
    let mut config = RedisClientConfig {
        endpoints: vec!["redis://one:6379".into(), "rediss://two:6380".into()],
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Cluster,
            sentinel: None,
            cluster: None,
        }),
        ..minimal_config()
    };
    assert!(config
        .validation_errors()
        .join("; ")
        .contains("cannot mix TLS and plaintext transports"));

    config.tls = Some(TlsConfig {
        enabled: true,
        ..Default::default()
    });
    assert!(config.validation_errors().is_empty());
}

#[test]
fn redis_timeout_defaults_are_explicit_and_bounded() {
    let config = minimal_config();

    assert_eq!(config.connect_timeout().unwrap(), REDIS_DEFAULT_CONNECT_TIMEOUT);
    assert_eq!(config.command_timeout().unwrap(), REDIS_DEFAULT_COMMAND_TIMEOUT);
}

#[test]
fn redis_auth_requires_secret_ref() {
    let yaml = r#"
secretRef:
  name: redis-creds
"#;
    let auth: RedisAuth = serde_yaml::from_str(yaml).expect("should deserialize");
    assert_eq!(auth.slot.secret_ref.name, "redis-creds");
    assert!(auth.slot.secret_ref.namespace.is_none());
}

#[test]
fn redis_auth_with_namespace() {
    let yaml = r#"
secretRef:
  name: redis-creds
  namespace: credentials
"#;
    let auth: RedisAuth = serde_yaml::from_str(yaml).expect("should deserialize");
    assert_eq!(auth.slot.secret_ref.name, "redis-creds");
    assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("credentials"));
}

#[test]
fn redis_auth_rejects_inline_password() {
    let yaml = "secretRef:\n  name: r\npassword: leaked";
    let r: Result<RedisAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
}

#[test]
fn redis_auth_rejects_inline_username() {
    let yaml = "secretRef:\n  name: r\nusername: admin";
    let r: Result<RedisAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
}

#[test]
fn redis_auth_rejects_missing_secret_ref() {
    let yaml = "{}";
    let r: Result<RedisAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err(), "secretRef is a required field");
}

#[test]
fn redis_auth_yaml_roundtrip_unchanged() {
    // Verify #[serde(flatten)] preserves the YAML shape at the parent level.
    let yaml = "secretRef:\n  name: redis-creds\n  namespace: default";
    let auth: super::RedisAuth = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(auth.slot.secret_ref.name, "redis-creds");
    assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("default"));
    assert!(auth.slot.secret.is_none());
}

#[test]
fn redis_auth_rejects_unknown_field() {
    let yaml = "secretRef:\n  name: x\nbogus: 1";
    let r: Result<super::RedisAuth, _> = serde_yaml::from_str(yaml);
    assert!(r.is_err());
}

#[test]
fn redis_sentinel_ignores_stale_password() {
    // No deny_unknown_fields on RedisSentinel: a legacy `password:` from a
    // pre-upgrade CR must be silently ignored, not rejected (graceful upgrade).
    let yaml = "masterName: mymaster\nsentinels:\n  - redis://h:26379\npassword: legacy-no-op";
    let s: super::RedisSentinel = serde_yaml::from_str(yaml).expect("stale password must be ignored");
    assert_eq!(s.master_name, "mymaster");
    assert_eq!(s.sentinels.len(), 1);
}

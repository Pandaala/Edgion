use super::*;
use crate::rate_limit_local_config::set_gateway_instance_count;
use parking_lot::Mutex;
use std::sync::OnceLock;

static GATEWAY_INSTANCE_COUNT_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn with_gateway_instance_count_lock<T>(f: impl FnOnce() -> T) -> T {
    let _guard = GATEWAY_INSTANCE_COUNT_TEST_LOCK.get_or_init(|| Mutex::new(())).lock();
    let result = f();
    set_gateway_instance_count(1);
    result
}

#[test]
fn test_default_config() {
    let config = RateLimitLocalConfig::default();
    assert_eq!(config.rate, 100);
    assert_eq!(config.interval, "1s");
    assert_eq!(config.reject_status, 429);
    assert!(config.show_limit_headers);
    assert!(config.estimator_slots_k.is_none());
}

#[test]
fn test_validation_rate() {
    let mut config = RateLimitLocalConfig {
        rate: 0,
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("rate"));
}

#[test]
fn test_validation_key_name_empty() {
    let mut config = RateLimitLocalConfig {
        rate: 10,
        key: vec![KeyGet::Header { name: "".to_string() }],
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("key[0].name"));
}

#[test]
fn test_validation_success() {
    let mut config = RateLimitLocalConfig {
        rate: 10,
        ..Default::default()
    };
    config.validate().expect("validate should succeed");
    assert_eq!(config.get_interval_duration(), Duration::from_secs(1));
}

#[test]
fn reject_status_uses_final_response_range() {
    for reject_status in [100, 101, 199, 600] {
        let config = RateLimitLocalConfig {
            reject_status,
            ..Default::default()
        };
        assert!(config.validation_error().is_some(), "{reject_status}");
    }
    for reject_status in [200, 599] {
        let config = RateLimitLocalConfig {
            reject_status,
            ..Default::default()
        };
        assert!(config.validation_error().is_none(), "{reject_status}");
    }
}

#[test]
fn test_parse_duration() {
    assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
    assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
    assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
    // Non-GEP-2257 forms (days, decimals) are rejected.
    assert!(parse_duration("1d").is_err());
    assert!(parse_duration("1.5s").is_err());
}

#[test]
fn test_interval_parsing() {
    let mut config = RateLimitLocalConfig {
        rate: 10,
        interval: "5s".to_string(),
        ..Default::default()
    };
    config.validate().expect("validate should succeed");
    assert_eq!(config.interval_duration, Some(Duration::from_secs(5)));
}

#[test]
fn test_limit_header_names() {
    let headers = LimitHeaderNames {
        limit: Some("X-Limit".to_string()),
        remaining: Some("".to_string()),
        reset: None,
        retry_in: Some("Retry-After".to_string()),
    };

    assert_eq!(headers.limit_header(), Some("X-Limit"));
    assert_eq!(headers.remaining_header(), None);
    assert_eq!(headers.reset_header(), None);
    assert_eq!(headers.retry_in_header(), Some("Retry-After"));
}

#[test]
fn test_limit_header_names_empty_string_normalized_on_deserialize() {
    // Empty strings must be normalized to None at deserialization time,
    // eliminating the dual representation of "not configured".
    let json = r#"{"limit":"X-Limit","remaining":"","reset":null}"#;
    let headers: LimitHeaderNames = serde_json::from_str(json).unwrap();
    assert_eq!(headers.limit, Some("X-Limit".to_string()));
    assert_eq!(headers.remaining, None);
    assert_eq!(headers.reset, None);
    assert_eq!(headers.retry_in, None);
}

#[test]
fn limit_header_names_validate_custom_names_and_omissions() {
    let headers = LimitHeaderNames {
        limit: Some("X-Limit".to_string()),
        remaining: Some("x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize)),
        reset: None,
        retry_in: Some("X-Retry-In".to_string()),
    };
    assert_eq!(headers.validation_error(), None);

    let omitted: LimitHeaderNames = serde_json::from_str(r#"{"limit":"","remaining":null}"#).unwrap();
    assert_eq!(omitted.validation_error(), None);
}

#[test]
fn limit_header_names_reject_invalid_and_oversized_names() {
    for name in [
        "bad header".to_string(),
        "héader".to_string(),
        "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize + 1),
    ] {
        let headers = LimitHeaderNames {
            limit: Some(name),
            ..Default::default()
        };
        assert_eq!(
            headers.validation_error().as_deref(),
            Some("headerNames.limit must be a valid HTTP header name of at most 256 bytes")
        );
    }
}

#[test]
fn limit_header_names_reject_duplicates_case_insensitively() {
    let headers = LimitHeaderNames {
        limit: Some("X-Quota".to_string()),
        remaining: Some("x-quota".to_string()),
        ..Default::default()
    };
    assert_eq!(
        headers.validation_error().as_deref(),
        Some("headerNames must not contain case-insensitive duplicate names")
    );
}

#[test]
fn limit_header_names_reject_framing_and_hop_by_hop_names() {
    for name in ["content-length", "proxy-connection"]
        .into_iter()
        .chain(crate::gwapi_types::HTTP_HOP_BY_HOP_HEADER_NAMES.iter().copied())
    {
        let headers = LimitHeaderNames {
            reset: Some(name.to_ascii_uppercase()),
            ..Default::default()
        };
        assert_eq!(
            headers.validation_error().as_deref(),
            Some("headerNames.reset must not use an HTTP framing or hop-by-hop header name"),
            "{name}"
        );
    }
}

#[test]
fn both_rate_limit_configs_apply_shared_header_name_validation() {
    let header_names = Some(LimitHeaderNames {
        limit: Some("transfer-encoding".to_string()),
        ..Default::default()
    });
    let local = RateLimitLocalConfig {
        header_names: header_names.clone(),
        ..Default::default()
    };
    let redis = super::super::rate_limit_redis::RateLimitRedisConfig {
        redis_ref: "default/shared-redis".to_string(),
        policies: vec![super::super::rate_limit_redis::RateLimitPolicy {
            rate: 100,
            interval: "1s".to_string(),
            algorithm: super::super::rate_limit_redis::RateLimitAlgorithm::default(),
            interval_duration: None,
        }],
        header_names,
        ..Default::default()
    };
    assert_eq!(local.validation_error(), redis.validation_error());
    assert!(local.validation_error().is_some());
}

#[test]
fn test_estimator_slots_default() {
    let mut config = RateLimitLocalConfig {
        rate: 10,
        ..Default::default()
    };
    config.validate().expect("validate should succeed");
    // Default should be 64K = 65536 (from global config)
    assert_eq!(config.get_effective_slots(), get_default_estimator_slots());
}

#[test]
fn test_estimator_slots_custom() {
    let mut config = RateLimitLocalConfig {
        rate: 10,
        estimator_slots_k: Some(16), // 16K = 16384 slots
        ..Default::default()
    };
    config.validate().expect("validate should succeed");
    assert_eq!(config.get_effective_slots(), 16 * SLOTS_K);
}

#[test]
fn test_estimator_slots_capped_at_max() {
    let mut config = RateLimitLocalConfig {
        rate: 10,
        estimator_slots_k: Some(2048), // 2048K, way over max 1024K
        ..Default::default()
    };
    // Global config: default=64K, max=256K for this test
    config
        .validate_with_global_config(64 * SLOTS_K, 256 * SLOTS_K)
        .expect("validate should succeed");
    assert_eq!(config.get_effective_slots(), 256 * SLOTS_K); // Capped at max
}

#[test]
fn test_estimator_slots_minimum() {
    let mut config = RateLimitLocalConfig {
        rate: 10,
        // Setting to 0 is not valid as a K value, but it will be converted to 0 slots
        // The min check will bump it up
        estimator_slots_k: Some(0),
        ..Default::default()
    };
    config.validate().expect("validate should succeed");
    assert_eq!(config.get_effective_slots(), get_min_estimator_slots()); // At least minimum (1K)
}

#[test]
fn test_yaml_key_format() {
    // Test YAML format with array of keys
    let yaml = r#"
rate: 100
interval: "1s"
key:
  - type: header
    name: "X-API-Key"
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.rate, 100);
    assert_eq!(
        config.key,
        vec![KeyGet::Header {
            name: "X-API-Key".to_string()
        }]
    );
}

#[test]
fn test_yaml_key_remote_ip() {
    // Test remoteIp key
    let yaml = r#"
rate: 50
key:
  - type: remoteIp
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.key, vec![KeyGet::RemoteIp]);
}

#[test]
fn test_yaml_key_remote_ip_and_path() {
    // Test remoteIpAndPath key
    let yaml = r#"
rate: 50
key:
  - type: remoteIpAndPath
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.key, vec![KeyGet::RemoteIpAndPath]);
}

#[test]
fn test_yaml_composite_keys() {
    // Test multiple keys combined
    let yaml = r#"
rate: 100
interval: "1s"
key:
  - type: remoteIp
  - type: header
    name: "X-API-Key"
  - type: path
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.rate, 100);
    assert_eq!(
        config.key,
        vec![
            KeyGet::RemoteIp,
            KeyGet::Header {
                name: "X-API-Key".to_string()
            },
            KeyGet::Path,
        ]
    );
}

#[test]
fn test_yaml_omitted_key_defaults_to_remote_ip() {
    // An omitted `key` must match the documented default and the `Default`
    // impl. It used to deserialize to an empty list, which the gateway then
    // treated as a key miss — with the default `onKeyMiss: Allow` that
    // silently disabled the limiter.
    let yaml = r#"
rate: 50
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.key, vec![KeyGet::RemoteIp]);
}

#[test]
fn test_yaml_explicit_empty_key_is_preserved() {
    // An explicitly empty list is a valid "one global bucket" policy and is
    // not rewritten to the default.
    let yaml = r#"
rate: 50
key: []
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.key.is_empty());
}

// ==================== Cluster Scope Tests ====================

#[test]
fn test_scope_default_is_instance() {
    let config = RateLimitLocalConfig::default();
    assert_eq!(config.scope, RateLimitLocalScope::Instance);
    assert_eq!(config.skew_tolerance, 1.2); // default skewTolerance
}

#[test]
fn test_effective_rate_instance_scope() {
    let config = RateLimitLocalConfig {
        rate: 100,
        ..Default::default()
    };
    // Instance scope: always returns configured rate, skewTolerance ignored
    assert_eq!(config.effective_rate_for_instances(4), 100);
}

#[test]
fn test_effective_rate_cluster_scope_with_default_skew() {
    with_gateway_instance_count_lock(|| {
        // Simulate 4 gateway instances
        set_gateway_instance_count(4);

        let config = RateLimitLocalConfig {
            rate: 1000,
            scope: RateLimitLocalScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };
        // ceil(1000 * 1.2 / 4) = ceil(300.0) = 300
        assert_eq!(config.get_effective_rate(), 300);
    });
}

#[test]
fn test_effective_rate_cluster_scope_no_skew() {
    with_gateway_instance_count_lock(|| {
        set_gateway_instance_count(4);

        let config = RateLimitLocalConfig {
            rate: 1000,
            scope: RateLimitLocalScope::Cluster,
            skew_tolerance: 1.0,
            ..Default::default()
        };
        // ceil(1000 * 1.0 / 4) = 250
        assert_eq!(config.get_effective_rate(), 250);
    });
}

#[test]
fn test_effective_rate_cluster_scope_high_skew() {
    with_gateway_instance_count_lock(|| {
        set_gateway_instance_count(3);

        let config = RateLimitLocalConfig {
            rate: 100,
            scope: RateLimitLocalScope::Cluster,
            skew_tolerance: 1.5,
            ..Default::default()
        };
        // ceil(100 * 1.5 / 3) = ceil(50.0) = 50
        assert_eq!(config.get_effective_rate(), 50);
    });
}

#[test]
fn test_effective_rate_cluster_scope_ceiling() {
    with_gateway_instance_count_lock(|| {
        set_gateway_instance_count(3);

        let config = RateLimitLocalConfig {
            rate: 10,
            scope: RateLimitLocalScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };
        // ceil(10 * 1.2 / 3) = ceil(4.0) = 4
        assert_eq!(config.get_effective_rate(), 4);
    });
}

#[test]
fn test_effective_rate_cluster_scope_fallback_to_one() {
    with_gateway_instance_count_lock(|| {
        // Default count is 1 (or controller unavailable)
        set_gateway_instance_count(1);

        let config = RateLimitLocalConfig {
            rate: 100,
            scope: RateLimitLocalScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };
        // ceil(100 * 1.2 / 1) = 120 (skewTolerance still applies)
        assert_eq!(config.get_effective_rate(), 120);
    });
}

#[test]
fn test_effective_rate_cluster_scope_minimum_one() {
    with_gateway_instance_count_lock(|| {
        set_gateway_instance_count(100);

        let config = RateLimitLocalConfig {
            rate: 2,
            scope: RateLimitLocalScope::Cluster,
            skew_tolerance: 1.0,
            ..Default::default()
        };
        // ceil(2 * 1.0 / 100) = ceil(0.02) = 1 (clamped to at least 1)
        assert_eq!(config.get_effective_rate(), 1);
    });
}

#[test]
fn test_cluster_scope_validation_low_rate() {
    let mut config = RateLimitLocalConfig {
        rate: 1,
        scope: RateLimitLocalScope::Cluster,
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("Cluster scope"));
}

#[test]
fn test_skew_tolerance_clamped_low() {
    let mut config = RateLimitLocalConfig {
        rate: 100,
        scope: RateLimitLocalScope::Cluster,
        skew_tolerance: 0.5, // Below minimum
        ..Default::default()
    };
    config.validate().expect("validate should succeed");
    assert_eq!(config.skew_tolerance, 1.0); // Clamped to 1.0
}

#[test]
fn test_skew_tolerance_clamped_high() {
    let mut config = RateLimitLocalConfig {
        rate: 100,
        scope: RateLimitLocalScope::Cluster,
        skew_tolerance: 3.0, // Above maximum
        ..Default::default()
    };
    config.validate().expect("validate should succeed");
    assert_eq!(config.skew_tolerance, 2.0); // Clamped to 2.0
}

#[test]
fn test_yaml_scope_cluster_with_skew() {
    let yaml = r#"
rate: 1000
interval: "1s"
scope: Cluster
skewTolerance: 1.5
key:
  - type: remoteIp
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.scope, RateLimitLocalScope::Cluster);
    assert_eq!(config.rate, 1000);
    assert_eq!(config.skew_tolerance, 1.5);
}

#[test]
fn test_yaml_scope_cluster_default_skew() {
    let yaml = r#"
rate: 1000
scope: Cluster
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.scope, RateLimitLocalScope::Cluster);
    assert_eq!(config.skew_tolerance, 1.2); // default
}

#[test]
fn test_yaml_scope_default_instance() {
    let yaml = r#"
rate: 100
"#;
    let config: RateLimitLocalConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.scope, RateLimitLocalScope::Instance);
}

#[test]
fn test_effective_rate_dynamic_count_change() {
    with_gateway_instance_count_lock(|| {
        // Start with 2 instances
        set_gateway_instance_count(2);

        let config = RateLimitLocalConfig {
            rate: 10,
            scope: RateLimitLocalScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };

        // effective_rate = ceil(10 * 1.2 / 2) = 6
        assert_eq!(config.get_effective_rate(), 6);

        // Scale up to 4 instances mid-flight
        set_gateway_instance_count(4);

        // effective_rate = ceil(10 * 1.2 / 4) = ceil(3.0) = 3
        assert_eq!(config.get_effective_rate(), 3);
    });
}

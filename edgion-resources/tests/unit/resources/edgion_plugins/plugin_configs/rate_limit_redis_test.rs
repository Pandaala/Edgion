use super::*;
use crate::redis_rate_limit::{MAX_RATE_LIMIT_UNITS, MAX_RATE_LIMIT_WINDOW_MS};

fn create_valid_config() -> RateLimitRedisConfig {
    RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![RateLimitPolicy {
                rate: 100,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            ..Default::default()
        }],
        redis_ref: "default/shared-redis".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_default_config() {
    let config = RateLimitRedisConfig::default();
    assert_eq!(config.reject_status, 429);
    assert!(config.show_limit_headers);
    assert_eq!(config.key_prefix, "edgion:rl:");
    assert_eq!(config.quotas[0].cost, 1);
    assert_eq!(config.max_key_len, 4096);
    assert_eq!(config.on_redis_failure, OnRedisFailure::Allow);
}

#[test]
fn test_valid_config() {
    for reject_status in [200, 599] {
        let mut config = create_valid_config();
        config.reject_status = reject_status;
        assert!(config.validate().is_ok(), "{reject_status}");
    }
}

#[test]
fn test_invalid_redis_ref_empty() {
    let mut config = create_valid_config();
    config.redis_ref = "".to_string();
    let err = config.validate().unwrap_err();
    assert!(err.contains("redis_ref"));
}

#[test]
fn test_invalid_redis_ref_no_slash() {
    let mut config = create_valid_config();
    config.redis_ref = "shared-redis".to_string();
    let err = config.validate().unwrap_err();
    assert!(err.contains("redis_ref"));
}

#[test]
fn test_invalid_redis_ref_special_chars() {
    let mut config = create_valid_config();
    config.redis_ref = "default/redis@cluster".to_string();
    assert!(config.validate().is_err());
}

#[test]
fn test_empty_policies() {
    let mut config = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![],
            ..Default::default()
        }],
        redis_ref: "default/redis".to_string(),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("At least one"));
}

#[test]
fn test_too_many_policies() {
    let mut config = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: (0..11)
                .map(|_| RateLimitPolicy {
                    rate: 100,
                    interval: "1s".to_string(),
                    algorithm: RateLimitAlgorithm::default(),
                    interval_duration: None,
                })
                .collect(),
            ..Default::default()
        }],
        redis_ref: "default/redis".to_string(),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(err.contains("Too many"));
}

#[test]
fn test_zero_rate_policy() {
    let mut config = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![RateLimitPolicy {
                rate: 0,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            ..Default::default()
        }],
        redis_ref: "default/redis".to_string(),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert_eq!(err, "policies[0].rate must be between 1 and 1000000");
}

#[test]
fn test_interval_too_small() {
    let mut config = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![RateLimitPolicy {
                rate: 100,
                interval: "50ms".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            ..Default::default()
        }],
        redis_ref: "default/redis".to_string(),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert_eq!(err, "policies[0].interval must be between 100ms and 2147483647ms");
}

#[test]
fn test_invalid_key_prefix_no_colon() {
    let mut config = create_valid_config();
    config.key_prefix = "edgion_rl".to_string();
    let err = config.validate().unwrap_err();
    assert!(err.contains("must end with ':'"));
}

#[test]
fn test_invalid_key_prefix_empty() {
    let mut config = create_valid_config();
    config.key_prefix = "".to_string();
    assert!(config.validate().is_err());
}

#[test]
fn test_zero_cost() {
    let mut config = create_valid_config();
    config.quotas[0].cost = 0;
    let err = config.validate().unwrap_err();
    assert!(err.contains("cost"));
}

#[test]
fn numeric_domain_accepts_maxima_and_rejects_first_invalid_values() {
    let mut config = create_valid_config();
    config.quotas[0].policies[0].rate = u64::from(MAX_RATE_LIMIT_UNITS);
    assert_eq!(MAX_RATE_LIMIT_WINDOW_MS, 2_147_483_647);
    config.quotas[0].policies[0].interval = "596h31m23s647ms".to_string();
    config.quotas[0].cost = MAX_RATE_LIMIT_UNITS;
    assert!(config.validate().is_ok());

    config.quotas[0].policies[0].rate += 1;
    assert_eq!(
        config.validate().unwrap_err(),
        "policies[0].rate must be between 1 and 1000000"
    );
    config.quotas[0].policies[0].rate = 1;
    config.quotas[0].policies[0].interval = "596h31m23s648ms".to_string();
    assert_eq!(
        config.validate().unwrap_err(),
        "policies[0].interval must be between 100ms and 2147483647ms"
    );
    config.quotas[0].policies[0].interval = "1s".to_string();
    config.quotas[0].cost = MAX_RATE_LIMIT_UNITS + 1;
    assert_eq!(config.validate().unwrap_err(), "cost must be between 1 and 1000000");
}

#[test]
fn numeric_schema_maxima_match_the_canonical_constant() {
    let policy_schema = serde_json::to_value(schemars::schema_for!(RateLimitPolicy)).unwrap();
    let config_schema = serde_json::to_value(schemars::schema_for!(RateLimitQuota)).unwrap();
    assert_eq!(
        policy_schema
            .pointer("/properties/rate/maximum")
            .and_then(|v| v.as_u64()),
        Some(u64::from(MAX_RATE_LIMIT_UNITS))
    );
    assert_eq!(
        config_schema
            .pointer("/properties/cost/maximum")
            .and_then(|v| v.as_u64()),
        Some(u64::from(MAX_RATE_LIMIT_UNITS))
    );
}

#[test]
fn test_max_key_len_too_small() {
    let mut config = create_valid_config();
    config.max_key_len = 32;
    let err = config.validate().unwrap_err();
    assert!(err.contains("max_key_len"));
}

#[test]
fn test_max_key_len_too_large() {
    let mut config = create_valid_config();
    config.max_key_len = 100_000;
    assert!(config.validate().is_err());
}

#[test]
fn test_multi_policy_config() {
    let mut config = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![
                RateLimitPolicy {
                    rate: 50,
                    interval: "1s".to_string(),
                    algorithm: RateLimitAlgorithm::SlidingWindow,
                    interval_duration: None,
                },
                RateLimitPolicy {
                    rate: 5000,
                    interval: "1h".to_string(),
                    algorithm: RateLimitAlgorithm::FixedWindow,
                    interval_duration: None,
                },
            ],
            ..Default::default()
        }],
        redis_ref: "prod/redis-cluster".to_string(),
        ..Default::default()
    };
    assert!(config.validate().is_ok());
    assert_eq!(
        config.quotas[0].policies[0].interval_duration,
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        config.quotas[0].policies[1].interval_duration,
        Some(Duration::from_secs(3600))
    );
}

#[test]
fn test_algorithm_key_prefix() {
    assert_eq!(RateLimitAlgorithm::SlidingWindow.key_prefix(), "sw");
    assert_eq!(RateLimitAlgorithm::FixedWindow.key_prefix(), "fw");
    assert_eq!(RateLimitAlgorithm::TokenBucket.key_prefix(), "tb");
}

#[test]
fn test_yaml_deserialization_basic() {
    let yaml = r#"
redisRef: default/shared-redis
quotas:
- name: default
  key:
  - type: remoteIp
  policies:
  - rate: 100
    interval: 1s
"#;
    let config: RateLimitRedisConfig = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(config.redis_ref, "default/shared-redis");
    assert_eq!(config.quotas[0].policies.len(), 1);
    assert_eq!(config.quotas[0].policies[0].rate, 100);
    assert_eq!(
        config.quotas[0].policies[0].algorithm,
        RateLimitAlgorithm::SlidingWindow
    );
}

#[test]
fn test_yaml_deserialization_multi_policy() {
    let yaml = r#"
redisRef: prod/redis
onRedisFailure: Deny
maxKeyLen: 16384
keyPrefix: 'myapp:rl:'
quotas:
- name: default
  policies:
  - rate: 50
    interval: 1s
    algorithm: SlidingWindow
  - rate: 5000
    interval: 1h
    algorithm: FixedWindow
  - rate: 100
    interval: 1m
    algorithm: TokenBucket
  onKeyMiss: Deny
  cost: 5
"#;
    let config: RateLimitRedisConfig = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(config.quotas[0].policies.len(), 3);
    assert_eq!(
        config.quotas[0].policies[0].algorithm,
        RateLimitAlgorithm::SlidingWindow
    );
    assert_eq!(config.quotas[0].policies[1].algorithm, RateLimitAlgorithm::FixedWindow);
    assert_eq!(config.quotas[0].policies[2].algorithm, RateLimitAlgorithm::TokenBucket);
    assert_eq!(config.on_redis_failure, OnRedisFailure::Deny);
    assert_eq!(config.quotas[0].on_key_miss, OnKeyMiss::Deny);
    assert_eq!(config.quotas[0].cost, 5);
    assert_eq!(config.max_key_len, 16384);
    assert_eq!(config.key_prefix, "myapp:rl:");
}

#[test]
fn test_invalid_reject_status() {
    for reject_status in [100, 101, 199, 600] {
        let mut config = create_valid_config();
        config.reject_status = reject_status;
        let err = config.validate().unwrap_err();
        assert!(err.contains("reject_status"), "{reject_status}: {err}");
    }
}

#[test]
fn reject_status_schema_matches_final_response_range() {
    let schema = serde_json::to_value(schemars::schema_for!(RateLimitRedisConfig)).unwrap();
    let status = &schema["properties"]["rejectStatus"];
    assert_eq!(status["minimum"].as_u64(), Some(200));
    assert_eq!(status["maximum"].as_u64(), Some(599));
}

#[test]
fn test_empty_key_name() {
    let mut config = create_valid_config();
    config.quotas[0].key = vec![KeyGet::Header { name: "".to_string() }];
    let err = config.validate().unwrap_err();
    assert!(err.contains("key[0].name"));
}

#[test]
fn test_on_redis_failure_default() {
    let config = RateLimitRedisConfig::default();
    assert_eq!(config.on_redis_failure, OnRedisFailure::Allow);
}

#[cfg(test)]
mod validation_trait_tests {
    use super::*;
    use crate::resources::edgion_plugins::PluginValidation;

    #[test]
    fn validation_error_returns_field_error_without_validate_call() {
        // invalid redis_ref should be caught without calling validate()
        let config = RateLimitRedisConfig {
            quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
                // missing slash
                policies: vec![RateLimitPolicy {
                    rate: 100,
                    interval: "1s".to_string(),
                    algorithm: RateLimitAlgorithm::default(),
                    interval_duration: None,
                }],
                ..Default::default()
            }],
            redis_ref: "invalid".to_string(),
            ..Default::default()
        };
        assert!(config.validation_error().is_some());
    }

    #[test]
    fn validation_error_does_not_treat_interval_duration_assignment_as_error() {
        // policy.interval_duration = Some(dur) is a side effect, not an error.
        // A valid config should return None even though interval_duration is None before validate().
        let config = RateLimitRedisConfig {
            quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
                policies: vec![RateLimitPolicy {
                    rate: 100,
                    interval: "1s".to_string(),
                    algorithm: RateLimitAlgorithm::default(),
                    interval_duration: None,
                }],
                ..Default::default()
            }],
            redis_ref: "default/shared-redis".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.validation_error(),
            None,
            "valid config with unset interval_duration must return None"
        );
    }
}

#[test]
fn request_timeout_parsed_into_duration() {
    let mut cfg = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![RateLimitPolicy {
                rate: 10,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::SlidingWindow,
                interval_duration: None,
            }],
            ..Default::default()
        }],
        redis_ref: "default/redis".to_string(),
        request_timeout: Some("500ms".to_string()),
        ..Default::default()
    };
    cfg.validate().unwrap();
    assert_eq!(cfg.request_timeout_duration, Some(Duration::from_millis(500)));
}

#[test]
fn rejects_unparseable_request_timeout() {
    let mut cfg = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![RateLimitPolicy {
                rate: 10,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::SlidingWindow,
                interval_duration: None,
            }],
            ..Default::default()
        }],
        redis_ref: "default/redis".to_string(),
        request_timeout: Some("nope".to_string()),
        ..Default::default()
    };
    assert!(cfg.validate().unwrap_err().contains("request_timeout"));
}

#[test]
fn rejects_zero_request_timeout() {
    let mut cfg = RateLimitRedisConfig {
        quotas: vec![crate::resources::edgion_plugins::RateLimitQuota {
            policies: vec![RateLimitPolicy {
                rate: 10,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::SlidingWindow,
                interval_duration: None,
            }],
            ..Default::default()
        }],
        redis_ref: "default/redis".to_string(),
        request_timeout: Some("0ms".to_string()),
        ..Default::default()
    };
    assert!(cfg.validate().unwrap_err().contains("request_timeout must be > 0"));
}

#[test]
fn multi_quota_validation_bounds_identity_and_latency() {
    let mut config = create_valid_config();
    let mut second = config.quotas[0].clone();
    second.name = "user".into();
    config.quotas.push(second);
    assert!(config.validation_error().unwrap().contains("requestTimeout"));
    config.request_timeout = Some("200ms".into());
    config.response_quota = Some("user".into());
    assert!(config.validate().is_ok());
    assert!(config.quotas.iter().all(|q| q.policies[0].interval_duration.is_some()));
    config.quotas[1].name = "default".into();
    assert!(config.validation_error().is_some());
    config.quotas[1].name = "invalid:name".into();
    assert!(config.validation_error().is_some());
    config.quotas[1].name = "user".into();
    config.response_quota = Some("absent".into());
    assert!(config.validation_error().is_some());
    config.response_quota = None;
    config.quotas = (0..7)
        .map(|i| {
            let mut quota = config.quotas[0].clone();
            quota.name = format!("q{i}");
            quota.policies = vec![quota.policies[0].clone(); 10];
            quota
        })
        .collect();
    assert!(config.validation_error().unwrap().contains("64"));
    config.quotas = (0..17)
        .map(|i| {
            let mut quota = config.quotas[0].clone();
            quota.name = format!("q{i}");
            quota
        })
        .collect();
    assert!(config.validation_error().unwrap().contains("16"));
}

#[test]
fn named_quota_schema_rejects_stale_flat_fields_and_preserves_key_defaults() {
    let value = serde_json::json!({"redisRef":"default/redis","quotas":[{"name":"main","policies":[{"rate":1,"interval":"1s"}]}]});
    let config: RateLimitRedisConfig = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(config.quotas[0].key, vec![KeyGet::RemoteIp]);
    for field in ["key", "policies", "cost", "onKeyMiss"] {
        let mut stale = value.clone();
        stale[field] = serde_json::json!(null);
        assert!(serde_json::from_value::<RateLimitRedisConfig>(stale).is_err());
    }
    let mut value = value;
    value["quotas"][0]["key"] = serde_json::json!([]);
    let config: RateLimitRedisConfig = serde_json::from_value(value.clone()).unwrap();
    assert!(config.quotas[0].key.is_empty());
    value["quotas"][0]["typo"] = serde_json::json!(1);
    assert!(serde_json::from_value::<RateLimitRedisConfig>(value).is_err());
}

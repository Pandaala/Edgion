use super::*;
use crate::redis_rate_limit::{MAX_RATE_LIMIT_UNITS, MAX_RATE_LIMIT_WINDOW_MS};

fn ip_dim(rate: u64, interval: &str) -> PerSourceIpLimit {
    PerSourceIpLimit {
        rate,
        interval: interval.to_string(),
        on_redis_failure: OnRedisFailure::Allow,
        interval_duration: None,
    }
}

#[test]
fn rejects_when_no_dimension_set() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: Some("default/redis".to_string()),
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: None,
        per_source_ip: None,
        redis_timeout: default_redis_timeout(),
    };
    assert_eq!(
        cfg.validation_error(),
        Some("At least one of 'perListener' or 'perSourceIp' must be set".to_string())
    );
}

#[test]
fn rejects_zero_rate() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: Some("default/redis".to_string()),
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: None,
        per_source_ip: Some(ip_dim(0, "1s")),
        redis_timeout: default_redis_timeout(),
    };
    assert_eq!(
        cfg.validation_error().unwrap(),
        "perSourceIp.rate must be between 1 and 1000000"
    );
}

#[test]
fn rejects_interval_below_100ms() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: Some("default/redis".to_string()),
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: None,
        per_source_ip: Some(ip_dim(10, "50ms")),
        redis_timeout: default_redis_timeout(),
    };
    assert_eq!(
        cfg.validation_error().unwrap(),
        "perSourceIp.interval must be between 100ms and 2147483647ms"
    );
}

#[test]
fn requires_redis_ref_when_redis_dimension_enabled() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: None,
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: None,
        per_source_ip: Some(ip_dim(10, "1s")),
        redis_timeout: default_redis_timeout(),
    };
    assert!(cfg.validation_error().unwrap().contains("redisRef is required"));
}

#[test]
fn rejects_bad_redis_ref_format() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: Some("no-slash".to_string()),
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: None,
        per_source_ip: Some(ip_dim(10, "1s")),
        redis_timeout: default_redis_timeout(),
    };
    assert!(cfg.validation_error().unwrap().contains("must be 'namespace/name'"));
}

#[test]
fn accepts_local_only_per_listener_without_redis() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: None,
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: Some(PerListenerLimit {
            rate: 10000,
            interval: "1s".to_string(),
            redis_distributed: false,
            on_redis_failure: OnRedisFailure::Deny,
            interval_duration: None,
        }),
        per_source_ip: None,
        redis_timeout: default_redis_timeout(),
    };
    assert_eq!(cfg.validation_error(), None);
}

#[test]
fn defaults_per_dimension_on_redis_failure() {
    let yaml = r#"
redisRef: "default/redis"
perListener:
  rate: 10000
  interval: "1s"
  redisDistributed: true
perSourceIp:
  rate: 50
  interval: "1s"
"#;
    let cfg: ConnectionRateLimitConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(cfg.per_listener.unwrap().on_redis_failure, OnRedisFailure::Deny);
    assert_eq!(cfg.per_source_ip.unwrap().on_redis_failure, OnRedisFailure::Allow);
}

#[test]
fn redis_timeout_defaults_to_100ms() {
    let yaml = r#"
redisRef: "default/redis"
perSourceIp:
  rate: 50
  interval: "1s"
"#;
    let cfg: ConnectionRateLimitConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(cfg.redis_timeout, "100ms");
}

#[test]
fn rejects_unparseable_redis_timeout() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: Some("default/redis".to_string()),
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: None,
        per_source_ip: Some(ip_dim(10, "1s")),
        redis_timeout: "garbage".to_string(),
    };
    assert!(cfg.validation_error().unwrap().contains("redisTimeout"));
}

#[test]
fn rejects_zero_redis_timeout() {
    let cfg = ConnectionRateLimitConfig {
        redis_ref: Some("default/redis".to_string()),
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: None,
        per_source_ip: Some(ip_dim(10, "1s")),
        redis_timeout: "0ms".to_string(),
    };
    assert!(cfg.validation_error().unwrap().contains("redisTimeout must be > 0"));
}

#[test]
fn numeric_domain_covers_local_and_redis_dimensions() {
    let mut cfg = ConnectionRateLimitConfig {
        redis_ref: Some("default/redis".to_string()),
        algorithm: RateLimitAlgorithm::TokenBucket,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: Some(PerListenerLimit {
            rate: u64::from(MAX_RATE_LIMIT_UNITS),
            interval: "596h31m23s647ms".to_string(),
            redis_distributed: true,
            on_redis_failure: OnRedisFailure::Deny,
            interval_duration: None,
        }),
        per_source_ip: Some(ip_dim(u64::from(MAX_RATE_LIMIT_UNITS), "100ms")),
        redis_timeout: default_redis_timeout(),
    };
    assert_eq!(cfg.validation_error(), None);

    cfg.per_listener.as_mut().unwrap().rate += 1;
    assert_eq!(
        cfg.validation_error().unwrap(),
        "perListener.rate must be between 1 and 1000000"
    );
    cfg.per_listener.as_mut().unwrap().rate = 1;
    assert_eq!(MAX_RATE_LIMIT_WINDOW_MS, 2_147_483_647);
    cfg.per_listener.as_mut().unwrap().interval = "596h31m23s648ms".to_string();
    assert_eq!(
        cfg.validation_error().unwrap(),
        "perListener.interval must be between 100ms and 2147483647ms"
    );
}

#[test]
fn numeric_schema_maxima_match_the_canonical_constant() {
    for schema in [
        serde_json::to_value(schemars::schema_for!(PerListenerLimit)).unwrap(),
        serde_json::to_value(schemars::schema_for!(PerSourceIpLimit)).unwrap(),
    ] {
        assert_eq!(
            schema.pointer("/properties/rate/maximum").and_then(|v| v.as_u64()),
            Some(u64::from(MAX_RATE_LIMIT_UNITS))
        );
    }
}

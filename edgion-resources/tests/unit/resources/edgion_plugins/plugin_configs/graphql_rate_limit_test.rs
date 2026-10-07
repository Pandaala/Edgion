use super::*;

#[test]
fn defaults_and_canonical_quota_mapping() {
    let config: GraphqlRateLimitConfig = serde_json::from_value(serde_json::json!({
        "schema": "type Query { ok: Boolean }", "redisRef": "default/redis",
        "policies": [{"rate":100, "interval":"1s"}]
    }))
    .unwrap();
    assert_eq!(config.max_cost, 10_000);
    assert_eq!(config.max_depth, 20);
    assert_eq!(config.request_timeout, "200ms");
    assert_eq!(config.on_redis_failure, OnRedisFailure::Deny);
    let redis = config.rate_limit_config();
    assert!(matches!(redis.on_key_miss, OnKeyMiss::Deny));
    assert_eq!(redis.key_prefix, "edgion:graphql:");
    assert_eq!(redis.reject_status, 429);
    assert_eq!(redis.cost, 1);
    assert!(config.compile().is_ok());
}

#[test]
fn body_policy_is_stricter_than_generic_json() {
    let body: BodyRequirement = serde_json::from_value(serde_json::json!({"maxBodySize":"65KiB"})).unwrap();
    assert!(graphql_body_validation_errors(&body)
        .iter()
        .any(|e| e.contains("64KiB")));
}

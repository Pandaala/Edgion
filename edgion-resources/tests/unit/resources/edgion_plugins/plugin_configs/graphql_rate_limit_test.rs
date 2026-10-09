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
    assert!(matches!(redis.quotas[0].on_key_miss, OnKeyMiss::Deny));
    assert_eq!(redis.key_prefix, "edgion:graphql:");
    assert_eq!(redis.reject_status, 429);
    assert_eq!(redis.quotas[0].cost, 1);
    assert!(config.compile().is_ok());
}

#[test]
fn body_policy_is_stricter_than_generic_json() {
    let body: BodyRequirement = serde_json::from_value(serde_json::json!({"maxBodySize":"65KiB"})).unwrap();
    assert!(graphql_body_validation_errors(&body)
        .iter()
        .any(|e| e.contains("64KiB")));
}

#[test]
fn shared_validation_rejects_ineffective_leaf_multipliers() {
    let mut config: GraphqlRateLimitConfig = serde_json::from_value(serde_json::json!({
        "schema": "type Query { ids(first:Int=100):[Int] }",
        "costRules": [{"field":"Query.ids", "multipliers":["first"]}],
        "redisRef": "default/redis", "policies": [{"rate":100, "interval":"1s"}]
    }))
    .unwrap();
    let error = config.compile().expect_err("leaf multipliers must be rejected");
    assert_eq!(config.validation_error(), Some(error));
    config.cost_rules[0].multipliers.clear();
    assert!(config.compile().is_ok());
    assert_eq!(config.validation_error(), None);
}

#[test]
fn root_quotas_validate_binding_shared_policy_limits_and_identity_inheritance() {
    let mut config: GraphqlRateLimitConfig = serde_json::from_value(serde_json::json!({
        "schema":"schema {query:R mutation:R} type R {ok:Int} type Child {value:Int}",
        "redisRef":"default/redis", "key":[{"type":"header","name":"X-Total"}],
        "policies":[{"rate":100,"interval":"1m"}],
        "rootQuotas":[{"name":"root","field":"R.ok","policies":[{"rate":10,"interval":"1m"}]}]
    }))
    .unwrap();
    assert!(config.compile().is_ok());
    let common = config.rate_limit_config();
    assert_eq!(common.quotas[1].key, config.key);
    assert_eq!(common.response_quota.as_deref(), Some("total"));
    assert_eq!(common.request_timeout.as_deref(), Some("200ms"));
    config.root_quotas[0].key = Some(vec![]);
    assert!(config.rate_limit_config().quotas[1].key.is_empty());
    config.response_quota = Some("root".into());
    assert!(config.compile().is_ok());
    for field in ["Child.value", "R.absent", "R.__typename"] {
        config.root_quotas[0].field = field.into();
        assert!(config.compile().is_err());
    }
    config.root_quotas[0].field = "R.ok".into();
    config.root_quotas[0].name = "total".into();
    assert!(config.compile().is_err());
    config.root_quotas[0].name = "root".into();
    config.root_quotas = vec![config.root_quotas[0].clone(); 16];
    assert!(config.compile().is_err());
}

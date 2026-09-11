use edgion_resources::resources::edgion_plugins::conditions::{
    Condition, ConditionSet, ConditionValidationProfile, PluginConditions,
};
use edgion_resources::resources::edgion_plugins::{EdgionPlugins, EdgionPluginsSpec, OnLimitExceeded, RateLimitConfig};
use serde_json::{json, Value};

fn local() -> Value {
    json!({"type":"RateLimit", "alias":"local", "config":{"rate":50,"key":[],"onLimitExceeded":"Continue"}})
}

fn redis() -> Value {
    json!({"type":"RateLimitRedis","config":{"redisRef":"default/redis","key":[],"policies":[{"rate":100,"interval":"1s"}]},
        "conditions":{"run":{"allOf":[{"type":"rateLimitExceeded","pluginAlias":"local"}]}}})
}

fn resource(entries: Vec<Value>) -> EdgionPlugins {
    EdgionPlugins {
        metadata: Default::default(),
        spec: serde_json::from_value::<EdgionPluginsSpec>(json!({"requestPlugins":entries})).unwrap(),
        status: None,
    }
}

#[test]
fn overflow_schema_is_strict_and_preserves_operator_fields() {
    let default: RateLimitConfig = serde_json::from_value(json!({"rate":50})).unwrap();
    assert_eq!(default.on_limit_exceeded, OnLimitExceeded::Reject);
    for action in ["continue", "Allow", "invalid"] {
        assert!(serde_json::from_value::<RateLimitConfig>(json!({"rate":50,"onLimitExceeded":action})).is_err());
    }
    let original = resource(vec![local(), redis()]);
    let encoded = serde_json::to_value(&original).unwrap();
    assert_eq!(
        encoded["spec"]["requestPlugins"][0]["config"]["onLimitExceeded"],
        "Continue"
    );
    assert_eq!(
        encoded["spec"]["requestPlugins"][1]["conditions"]["run"]["allOf"][0]["pluginAlias"],
        "local"
    );
    assert!(!encoded.to_string().contains("descriptor"));
    assert!(original.validate_plugins().is_empty());
    for field in ["negate", "pluginAliaz", "descriptor"] {
        let mut value = json!({"type":"rateLimitExceeded","pluginAlias":"local"});
        value[field] = json!(true);
        assert!(serde_json::from_value::<Condition>(value).is_err(), "{field}");
    }
    for alias in [
        "",
        " ",
        "resource/local",
        "local@x",
        "abcdefghijklmnopqrstuvwxyz123456789",
    ] {
        let mut consumer = redis();
        consumer["conditions"]["run"]["allOf"][0]["pluginAlias"] = json!(alias);
        assert!(
            !resource(vec![local(), consumer]).validate_plugins().is_empty(),
            "{alias}"
        );
    }
}

#[test]
fn overflow_dependencies_reject_invalid_targets() {
    let mut reject = local();
    reject["config"]["onLimitExceeded"] = json!("Reject");
    let mut disabled = local();
    disabled["enable"] = json!(false);
    let mut self_ref = redis();
    self_ref["alias"] = json!("local");
    for entries in [
        vec![redis()],
        vec![redis(), local()],
        vec![local(), local(), redis()],
        vec![reject, redis()],
        vec![disabled, redis()],
        vec![self_ref],
        vec![json!({"type":"Cors","alias":"local","config":{}}), redis()],
    ] {
        let errors = resource(entries).validate_plugins();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("rateLimitExceeded.pluginAlias")),
            "{errors:?}"
        );
    }
}

#[test]
fn overflow_dormant_consumers_are_inert_and_continue_needs_no_consumer() {
    assert!(resource(vec![local()]).validate_plugins().is_empty());
    let mut disabled = redis();
    disabled["enable"] = json!(false);
    assert!(resource(vec![disabled]).validate_plugins().is_empty());
}

#[test]
fn overflow_predicate_has_one_permitted_placement() {
    let predicate: Condition =
        serde_json::from_value(json!({"type":"rateLimitExceeded","pluginAlias":"local"})).unwrap();
    for profile in [
        ConditionValidationProfile::EntrySync,
        ConditionValidationProfile::RequestAsync,
    ] {
        assert!(predicate.validate_for_profile(profile).is_err());
    }
    let conditions = PluginConditions {
        skip: Some(ConditionSet::all_of(vec![predicate.clone()])),
        run: None,
    };
    assert!(conditions.validate_for_rate_limit_redis().is_err());
    let conditions = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![predicate])),
    };
    assert!(conditions.validate_for_rate_limit_redis().is_ok());
    assert!(conditions.validate().is_err());
    let mut wrong_type = local();
    wrong_type["conditions"] = redis()["conditions"].clone();
    assert!(!resource(vec![wrong_type]).validate_plugins().is_empty());
    let mut response = resource(vec![]);
    response.spec.upstream_response_plugins = Some(serde_json::from_value(json!([redis()])).unwrap());
    assert!(response
        .validate_plugins()
        .iter()
        .any(|error| error.contains("rateLimitExceeded is only valid")));
}

#[test]
fn overflow_redis_profile_keeps_hmac_body_restriction() {
    let condition: Condition = serde_yaml::from_str("type: hmacAuth\nsecretGroups:\n  - name: default\n    secretRefs:\n      - name: credentials\nalgorithms: [hmac-sha256]\nvalidateRequestBody: true\n").unwrap();
    let errors = condition
        .validate_for_profile(ConditionValidationProfile::RateLimitRedisRun)
        .unwrap_err();
    assert!(errors.contains("validateRequestBody"), "{errors}");
}

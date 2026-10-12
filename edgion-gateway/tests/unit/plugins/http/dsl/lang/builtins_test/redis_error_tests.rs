use super::{
    bounded_redis_get_limit, redis_dependency_err, redis_err, redis_hash_result, PluginLog, RedisFailure, Value,
};

#[test]
fn oversized_hash_uses_existing_bounded_failure_map() {
    for (limit, expected) in [(16, "result_too_large"), (1, "e"), (0, "")] {
        let mut log = PluginLog::new("test");
        let result = redis_hash_result(None, limit, &mut log);
        assert_eq!(error_text(&result), expected);
        let Value::Map(map) = result else {
            panic!("expected Redis envelope")
        };
        assert_eq!(map.get("ok"), Some(&Value::Bool(false)));
        assert_eq!(map.get("value"), Some(&Value::Nil));
        assert!(log.contains("redis result too large"));
    }
}

#[test]
fn admitted_hash_preserves_utf8_truncation_and_success_envelope() {
    let mut log = PluginLog::new("test");
    let result = redis_hash_result(
        Some(std::collections::HashMap::from([("aé".into(), "bñ".into())])),
        2,
        &mut log,
    );
    let Value::Map(map) = result else {
        panic!("expected Redis envelope")
    };
    assert_eq!(map.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(map.get("error"), Some(&Value::Nil));
    let Some(Value::Map(hash)) = map.get("value") else {
        panic!("expected Redis hash")
    };
    assert_eq!(hash.get("a"), Some(&Value::Str("b".into())));
}

fn error_text(value: &Value) -> String {
    let Value::Map(map) = value else {
        panic!("expected Redis result map");
    };
    let Some(Value::Str(error)) = map.get("error") else {
        panic!("expected Redis error string");
    };
    error.to_string()
}

#[test]
fn fixed_error_tags_never_exceed_the_string_budget() {
    for failure in RedisFailure::ALL {
        let tag = failure.tag();
        assert_eq!(error_text(&redis_err(failure, tag.len())), tag);
        assert_eq!(error_text(&redis_err(failure, 1)), "e");
        assert_eq!(error_text(&redis_err(failure, 0)), "");
    }
}

#[test]
fn dependency_detail_is_not_script_visible_and_diagnostic_is_bounded() {
    for sentinel in [
        "alias-sentinel".repeat(64),
        "key-sentinel".repeat(64),
        "field-sentinel".repeat(64),
        "credential-and-address-sentinel".repeat(32),
    ] {
        let error = anyhow::anyhow!("Redis command failed: {sentinel}");
        let mut log = PluginLog::new("test");

        let value = redis_dependency_err(&error, 16, &mut log);

        assert_eq!(error_text(&value), "dependency_error");
        assert!(!format!("{value:?}").contains(&sentinel));
        assert!(log.contains("err:"));
        assert!(log.render_for_test().len() <= 128);
    }
}

#[test]
fn invalid_response_is_a_fixed_bounded_public_class() {
    assert_eq!(
        error_text(&redis_err(RedisFailure::InvalidResponse, 16)),
        "invalid_response"
    );
    assert_eq!(error_text(&redis_err(RedisFailure::InvalidResponse, 1)), "e");
    assert_eq!(RedisFailure::ALL.len(), 8);
}

#[test]
fn bounded_get_cap_admission_is_independent_of_provider_io() {
    assert_eq!(bounded_redis_get_limit(&Value::Int(65536), 65536), Some(65536));
    assert_eq!(bounded_redis_get_limit(&Value::Int(65537), 65536), Some(65536));
    assert_eq!(bounded_redis_get_limit(&Value::Int(i64::MAX), 65536), Some(65536));
    assert_eq!(bounded_redis_get_limit(&Value::Int(65536), 8), Some(8));
    for value in [Value::Nil, Value::Int(0), Value::Int(-1), Value::Str("8".into())] {
        assert_eq!(bounded_redis_get_limit(&value, 65536), None);
    }
}

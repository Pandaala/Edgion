use super::{redis_dependency_err, redis_err, PluginLog, RedisFailure, Value};

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

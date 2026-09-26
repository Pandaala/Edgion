use super::*;
use edgion_resources::resources::common::external_http::MAX_OUTBOUND_TIMEOUT_MS;
use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::{
    DSL_MAX_BODY_SCAN_PASSES, DSL_MAX_CALL_COUNT, DSL_MAX_HTTP_REQUEST_BODY_BYTES, DSL_MAX_HTTP_RESPONSE_BODY_BYTES,
    DSL_MAX_LIST_LEN, DSL_MAX_LOOP_ITERATIONS, DSL_MAX_MAP_SIZE, DSL_MAX_STACK_DEPTH, DSL_MAX_STEPS,
    DSL_MAX_STRING_LEN,
};

fn config(source: &str) -> DslConfig {
    DslConfig {
        name: "body-test".to_string(),
        source: Some(source.to_string()),
        ..Default::default()
    }
}

fn config_with_name(name: String) -> DslConfig {
    DslConfig {
        name,
        source: Some("return next()".to_string()),
        ..Default::default()
    }
}

#[test]
fn builtin_argument_capacity_has_controller_gateway_parity() {
    for count in [255, 256, 257] {
        let source = format!("log_fmt(\"{}\"{})", "{}".repeat(count - 1), ", nil".repeat(count - 1));
        let config = config(&source);
        if count == 255 {
            assert!(config.compile_and_validate().is_ok());
            assert!(DslPlugin::from_config(&config).is_some());
            assert!(DslPlugin::from_config_response(&config).is_some());
        } else {
            let error = config.compile_and_validate().unwrap_err();
            assert!(
                error.contains("builtin call exceeds maximum argument count (255)"),
                "{error}"
            );
            assert_eq!(DslPlugin::resolve_parts(&config).err().unwrap(), error);
            assert!(DslPlugin::from_config(&config).is_none());
            assert!(DslPlugin::from_config_response(&config).is_none());
        }
    }
}

#[test]
fn truncated_builtin_bytecode_is_rejected_by_both_factories() {
    use edgion_dsl::bytecode::{BuiltinId, OpCode};

    let mut config = config(&format!("log_fmt(\"{}\"{})", "{}".repeat(254), ", nil".repeat(254)));
    let original = config.compile_and_validate().unwrap();
    for argc in [0, 1] {
        let mut script = original.clone();
        for op in &mut script.code {
            if let OpCode::CallBuiltin(BuiltinId::LogFmt, count) = op {
                *count = argc;
            }
        }
        // Bytecode has priority even when valid source is also supplied.
        config.bytecode = Some(script.serialize_base64().unwrap());
        let error = config.compile_and_validate().unwrap_err();
        assert!(
            // argc 1 reads the last `nil` argument as the template.
            error.contains(if argc == 0 {
                "expects 1 to 255"
            } else {
                "must come directly from a string constant"
            }),
            "{error}"
        );
        assert_eq!(DslPlugin::resolve_parts(&config).err().unwrap(), error);
        assert!(DslPlugin::from_config(&config).is_none());
        assert!(DslPlugin::from_config_response(&config).is_none());
    }
}

#[test]
fn dynamic_log_template_bytecode_is_rejected_by_both_factories() {
    use edgion_dsl::bytecode::{BuiltinId, CompiledScript, Constant, OpCode, BYTECODE_VERSION};

    let mut config = config("log_fmt(\"static\")");
    let valid = config.compile_and_validate().unwrap().serialize_base64().unwrap();
    config.bytecode = Some(valid);
    assert!(config.compile_and_validate().is_ok());
    assert!(DslPlugin::from_config(&config).is_some());
    assert!(DslPlugin::from_config_response(&config).is_some());

    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![
            OpCode::LoadConst(0),
            OpCode::CallBuiltin(BuiltinId::ReqHeader, 1),
            OpCode::CallBuiltin(BuiltinId::LogFmt, 1),
            OpCode::Pop,
            OpCode::ReturnNext,
        ],
        constants: vec![Constant::Str("X-Token".into())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    config.bytecode = Some(script.serialize_base64().unwrap());
    let error = config.compile_and_validate().unwrap_err();
    assert!(
        error.contains("log_fmt template") && error.contains("string constant"),
        "{error}"
    );
    assert_eq!(DslPlugin::resolve_parts(&config).err().unwrap(), error);
    assert!(DslPlugin::from_config(&config).is_none());
    assert!(DslPlugin::from_config_response(&config).is_none());
}

#[test]
fn standalone_factories_enforce_plugin_log_name_capacity() {
    assert_eq!(DslConfig::MAX_NAME_BYTES, crate::plugins::runtime::log::NAME_CAPACITY);

    let valid = config_with_name("a".repeat(DslConfig::MAX_NAME_BYTES));
    let request_filter = DslPlugin::from_config(&valid).expect("36-byte request plugin name must be accepted");
    assert_eq!(request_filter.name(), valid.name);
    let response_filter =
        DslPlugin::from_config_response(&valid).expect("36-byte response plugin name must be accepted");
    assert_eq!(response_filter.name(), valid.name);

    let overlong = config_with_name("a".repeat(DslConfig::MAX_NAME_BYTES + 1));
    assert!(DslPlugin::from_config(&overlong).is_none());
    assert!(DslPlugin::from_config_response(&overlong).is_none());
}

#[test]
fn standalone_factories_reject_unbounded_runtime_limits() {
    for (field, limit_value) in [
        ("maxSteps", u64::from(DSL_MAX_STEPS) + 1),
        ("maxLoopIterations", u64::from(DSL_MAX_LOOP_ITERATIONS) + 1),
        ("maxCallCount", u64::from(DSL_MAX_CALL_COUNT) + 1),
        ("maxStackDepth", DSL_MAX_STACK_DEPTH as u64 + 1),
        ("maxStringLen", DSL_MAX_STRING_LEN as u64 + 1),
        ("maxListLen", DSL_MAX_LIST_LEN as u64 + 1),
        ("maxMapSize", DSL_MAX_MAP_SIZE as u64 + 1),
        ("maxBodyScanPasses", u64::from(DSL_MAX_BODY_SCAN_PASSES) + 1),
        ("httpMaxTimeoutMs", MAX_OUTBOUND_TIMEOUT_MS + 1),
        ("httpMaxResponseBodyBytes", DSL_MAX_HTTP_RESPONSE_BODY_BYTES as u64 + 1),
        ("httpMaxRequestBodyBytes", DSL_MAX_HTTP_REQUEST_BODY_BYTES as u64 + 1),
    ] {
        let mut config_value = serde_json::json!({
            "name": "factory-limits",
            "source": "return next()"
        });
        config_value[field] = serde_json::json!(limit_value);
        let config: DslConfig = serde_json::from_value(config_value).unwrap();
        assert!(
            DslPlugin::from_config(&config).is_none(),
            "request factory accepted invalid {field}"
        );
        assert!(
            DslPlugin::from_config_response(&config).is_none(),
            "response factory accepted invalid {field}"
        );
    }
}

#[test]
fn dsl_request_filter_uses_capture_only_body_gate() {
    let filter = DslPlugin::from_config(&config("let found = req.body.contains(\"x\")")).unwrap();
    assert!(!filter.requires_materialized_request_body());
}

#[test]
fn request_body_call_limit_resolves_default_and_explicit_values() {
    let (_, _, _, default_runtime) = DslPlugin::resolve_parts(&config("return next()")).unwrap();
    assert_eq!(default_runtime.max_request_body_bytes, 1024 * 1024);

    let mut explicit = config("return next()");
    explicit.http_max_request_body_bytes = Some(64 * 1024);
    let (_, _, _, explicit_runtime) = DslPlugin::resolve_parts(&explicit).unwrap();
    assert_eq!(explicit_runtime.max_request_body_bytes, 64 * 1024);
}

#[test]
fn response_factory_rejects_request_body_mutation() {
    assert!(DslPlugin::from_config_response(&config("req.body.clear()")).is_none());
}

#[test]
fn response_factory_allows_cached_request_body_reads() {
    assert!(DslPlugin::from_config_response(&config("let found = req.body.contains(\"x\")")).is_some());
}

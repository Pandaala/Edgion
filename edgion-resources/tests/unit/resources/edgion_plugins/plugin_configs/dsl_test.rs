use super::*;

fn config_with_limit(field: &str, value: u64) -> DslConfig {
    let mut config = serde_json::json!({
        "name": "limits",
        "source": "return next()"
    });
    config[field] = serde_json::json!(value);
    serde_json::from_value(config).unwrap()
}

#[test]
fn validation_error_passes_for_valid_config() {
    let config = DslConfig {
        name: "ok".to_string(),
        source: Some("let x = 1".to_string()),
        bytecode: None,
        ..Default::default()
    };
    // Full pipeline — the trivial script compiles, so no error.
    assert_eq!(config.validation_error(), None);
}

#[test]
fn runtime_limit_defaults_are_resolved_by_the_resource_contract() {
    let limits = DslConfig::default().resolve_runtime_limits().unwrap();
    assert_eq!(limits.max_steps, DSL_DEFAULT_MAX_STEPS);
    assert_eq!(limits.max_loop_iterations, DSL_DEFAULT_MAX_LOOP_ITERATIONS);
    assert_eq!(limits.max_call_count, DSL_DEFAULT_MAX_CALL_COUNT);
    assert_eq!(limits.max_stack_depth, DSL_DEFAULT_MAX_STACK_DEPTH);
    assert_eq!(limits.max_string_len, DSL_DEFAULT_MAX_STRING_LEN);
    assert_eq!(limits.max_list_len, DSL_DEFAULT_MAX_LIST_LEN);
    assert_eq!(limits.max_map_size, DSL_DEFAULT_MAX_MAP_SIZE);
    assert_eq!(limits.max_body_scan_passes, DSL_DEFAULT_MAX_BODY_SCAN_PASSES);
    assert_eq!(limits.http_max_timeout_ms, DSL_DEFAULT_HTTP_MAX_TIMEOUT_MS);
    assert_eq!(
        limits.http_max_response_body_bytes,
        DSL_DEFAULT_HTTP_MAX_RESPONSE_BODY_BYTES
    );
    assert_eq!(
        limits.http_max_request_body_bytes,
        DSL_DEFAULT_HTTP_MAX_REQUEST_BODY_BYTES
    );
}

#[test]
fn runtime_limit_contract_accepts_exact_minimum_and_maximum() {
    for (field, max) in [
        ("maxSteps", u64::from(DSL_MAX_STEPS)),
        ("maxLoopIterations", u64::from(DSL_MAX_LOOP_ITERATIONS)),
        ("maxCallCount", u64::from(DSL_MAX_CALL_COUNT)),
        ("maxStackDepth", DSL_MAX_STACK_DEPTH as u64),
        ("maxStringLen", DSL_MAX_STRING_LEN as u64),
        ("maxListLen", DSL_MAX_LIST_LEN as u64),
        ("maxMapSize", DSL_MAX_MAP_SIZE as u64),
        ("maxBodyScanPasses", u64::from(DSL_MAX_BODY_SCAN_PASSES)),
        ("httpMaxTimeoutMs", MAX_OUTBOUND_TIMEOUT_MS),
        ("httpMaxResponseBodyBytes", DSL_MAX_HTTP_RESPONSE_BODY_BYTES as u64),
        ("httpMaxRequestBodyBytes", DSL_MAX_HTTP_REQUEST_BODY_BYTES as u64),
    ] {
        for value in [1, max] {
            let config = config_with_limit(field, value);
            assert!(config.resolve_runtime_limits().is_ok(), "rejected {field}={value}");
            assert_eq!(config.validation_error(), None, "rejected {field}={value}");
            assert!(config.compile_and_validate().is_ok(), "rejected {field}={value}");
        }
    }
}

#[test]
fn runtime_limit_contract_rejects_zero_and_above_maximum() {
    for (field, max) in [
        ("maxSteps", u64::from(DSL_MAX_STEPS)),
        ("maxLoopIterations", u64::from(DSL_MAX_LOOP_ITERATIONS)),
        ("maxCallCount", u64::from(DSL_MAX_CALL_COUNT)),
        ("maxStackDepth", DSL_MAX_STACK_DEPTH as u64),
        ("maxStringLen", DSL_MAX_STRING_LEN as u64),
        ("maxListLen", DSL_MAX_LIST_LEN as u64),
        ("maxMapSize", DSL_MAX_MAP_SIZE as u64),
        ("maxBodyScanPasses", u64::from(DSL_MAX_BODY_SCAN_PASSES)),
        ("httpMaxTimeoutMs", MAX_OUTBOUND_TIMEOUT_MS),
        ("httpMaxResponseBodyBytes", DSL_MAX_HTTP_RESPONSE_BODY_BYTES as u64),
        ("httpMaxRequestBodyBytes", DSL_MAX_HTTP_REQUEST_BODY_BYTES as u64),
    ] {
        for value in [0, max + 1] {
            let config = config_with_limit(field, value);
            let expected = format!("{field} must be between 1 and {max}");
            assert_eq!(config.resolve_runtime_limits().unwrap_err(), expected);
            assert_eq!(config.validation_error(), Some(expected.clone()));
            assert_eq!(config.compile_and_validate().unwrap_err(), expected);
        }
    }
}

#[test]
fn runtime_limit_contract_rejects_u64_max_timeout_without_overflow() {
    let config = config_with_limit("httpMaxTimeoutMs", u64::MAX);
    assert_eq!(
        config.compile_and_validate().unwrap_err(),
        format!("httpMaxTimeoutMs must be between 1 and {MAX_OUTBOUND_TIMEOUT_MS}")
    );
}

#[test]
fn missing_script_precedes_runtime_limit_error() {
    let config = DslConfig {
        name: "dual".to_string(),
        max_steps: Some(0),
        ..Default::default()
    };
    assert_eq!(
        config.validation_error(),
        Some("either source or bytecode must be provided".to_string())
    );
    assert_eq!(
        config.compile_and_validate().unwrap_err(),
        "either source or bytecode must be provided"
    );
}

#[test]
fn runtime_limit_error_precedes_compile_error() {
    let config = DslConfig {
        name: "dual".to_string(),
        source: Some("this is not valid dsl @@@".to_string()),
        max_steps: Some(0),
        ..Default::default()
    };
    assert_eq!(
        config.validation_error(),
        Some("maxSteps must be between 1 and 1000000".to_string())
    );
    assert_eq!(
        config.compile_and_validate().unwrap_err(),
        "maxSteps must be between 1 and 1000000"
    );
}

#[test]
fn validation_error_requires_name() {
    let config = DslConfig {
        name: String::new(),
        source: Some("let x = 1".to_string()),
        ..Default::default()
    };
    assert_eq!(
        config.validation_error(),
        Some("dsl plugin name is required".to_string())
    );
    assert_eq!(
        config.compile_and_validate().unwrap_err(),
        "dsl plugin name is required"
    );
}

#[test]
fn plugin_name_uses_utf8_byte_boundaries() {
    for name in ["a".repeat(36), "é".repeat(18)] {
        let config = DslConfig {
            name,
            source: Some("let x = 1".to_string()),
            ..Default::default()
        };
        assert_eq!(config.validation_error(), None);
        assert!(config.compile_and_validate().is_ok());
    }

    for name in ["a".repeat(37), "é".repeat(19)] {
        let config = DslConfig {
            name,
            source: Some("let x = 1".to_string()),
            ..Default::default()
        };
        let expected = Some("dsl plugin name must be at most 36 UTF-8 bytes".to_string());
        assert_eq!(config.validation_error(), expected);
        assert_eq!(
            config.compile_and_validate().unwrap_err(),
            "dsl plugin name must be at most 36 UTF-8 bytes"
        );
    }
}

#[test]
fn validation_error_requires_source_or_bytecode() {
    let config = DslConfig {
        name: "x".to_string(),
        source: None,
        bytecode: None,
        ..Default::default()
    };
    assert!(config.validation_error().is_some());
}

#[test]
fn validation_error_rejects_out_of_range_deny_status() {
    for status in [99, 103, 600] {
        let config = DslConfig {
            name: "x".to_string(),
            source: Some("let x = 1".to_string()),
            error_policy: Some(DslErrorPolicy::DenyWith { status, body: None }),
            ..Default::default()
        };
        assert!(config.validation_error().is_some(), "{status}");
    }
}

#[test]
fn validation_error_rejects_out_of_range_body_scan_passes() {
    let config = DslConfig {
        name: "scan-budget".to_string(),
        source: Some("return next".to_string()),
        max_body_scan_passes: Some(0),
        ..Default::default()
    };
    assert_eq!(
        config.validation_error(),
        Some("maxBodyScanPasses must be between 1 and 16".to_string())
    );

    let config = DslConfig {
        max_body_scan_passes: Some(17),
        ..config
    };
    assert_eq!(
        config.validation_error(),
        Some("maxBodyScanPasses must be between 1 and 16".to_string())
    );
}

#[test]
fn validation_error_checks_http_request_body_limit_boundaries() {
    for bytes in [1, 33_554_432] {
        let config = DslConfig {
            name: "request-body-limit".to_string(),
            source: Some("return next()".to_string()),
            http_max_request_body_bytes: Some(bytes),
            ..Default::default()
        };
        assert_eq!(config.validation_error(), None);
    }

    for bytes in [0, 33_554_433] {
        let config = DslConfig {
            name: "request-body-limit".to_string(),
            source: Some("return next()".to_string()),
            http_max_request_body_bytes: Some(bytes),
            ..Default::default()
        };
        assert_eq!(
            config.validation_error(),
            Some("httpMaxRequestBodyBytes must be between 1 and 33554432".to_string())
        );
    }
}

#[test]
fn validation_error_rejects_uncompilable_source() {
    let config = DslConfig {
        name: "bad".to_string(),
        source: Some("this is not valid dsl @@@".to_string()),
        ..Default::default()
    };
    let err = config.validation_error().expect("uncompilable source must fail");
    assert!(err.contains("DSL compile errors"), "got: {err}");
}

#[test]
fn validation_error_rejects_unknown_redis_link() {
    let config = DslConfig {
        name: "unknown-link".to_string(),
        source: Some("let r = redis.get(\"mylink\", \"mykey\")".to_string()),
        ..Default::default()
    };
    let err = config
        .validation_error()
        .expect("redis call referencing an undeclared link must fail");
    assert!(err.contains("linkSysRefs"), "got: {err}");
}

#[test]
fn validation_error_rejects_streaming_upstream_response_body_read() {
    let config = DslConfig {
        name: "response-body".to_string(),
        source: Some("let body = resp.body".to_string()),
        ..Default::default()
    };
    let err = config
        .validation_error()
        .expect("proxied upstream response bodies must remain streaming-only");
    assert!(err.contains("streaming-only"), "got: {err}");
}

#[test]
fn compile_and_validate_compile_error_wins_on_dual_fault() {
    use crate::resources::edgion_plugins::plugin_configs::LinkSysRef;
    // Both faults present: uncompilable source AND an invalid linkSysRefs
    // name. The compile error must win (original gateway error priority).
    let config = DslConfig {
        name: "dual".to_string(),
        source: Some("this is not valid dsl @@@".to_string()),
        linksys_refs: vec![LinkSysRef {
            namespace: "default".to_string(),
            name: "BAD NAME".to_string(),
        }],
        ..Default::default()
    };
    let err = config.compile_and_validate().expect_err("dual fault must fail");
    assert!(err.contains("DSL compile errors"), "compile error must win, got: {err}");
}

#[test]
fn compile_and_validate_reports_link_format_error_when_script_compiles() {
    use crate::resources::edgion_plugins::plugin_configs::LinkSysRef;
    let config = DslConfig {
        name: "bad-link-name".to_string(),
        source: Some("let a = 1".to_string()),
        linksys_refs: vec![LinkSysRef {
            namespace: "default".to_string(),
            name: "BAD NAME".to_string(),
        }],
        ..Default::default()
    };
    let err = config.compile_and_validate().expect_err("invalid link name must fail");
    assert!(err.contains("invalid name"), "got: {err}");
}

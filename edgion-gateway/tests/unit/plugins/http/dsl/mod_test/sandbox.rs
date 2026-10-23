//! Tests for VM sandbox limits: step limit, loop limit, string length limit, error policy variants.

use super::*;

#[test]
fn test_step_limit_exceeded() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_steps: 5,
        ..Default::default()
    };
    // Uses run_expect_runtime_err_with_limits to assert the exact RuntimeError
    // variant instead of the lossy `status: 500` shape produced by DslErrorPolicy::Deny.
    let err = run_expect_runtime_err_with_limits(
        "let a=1\nlet b=2\nlet c=3\nlet d=4\nlet e=5\nlet f=6\nlet g=7\nlet h=8\nlet k=9\nlet l=10",
        session,
        limits,
    );
    assert!(
        matches!(err, RuntimeError::StepLimitExceeded { .. }),
        "expected StepLimitExceeded, got {err:?}"
    );
}

#[test]
fn test_loop_limit_exceeded() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_loop_iterations: 3,
        ..Default::default()
    };
    let source = r#"while true { }"#;
    let err = run_expect_runtime_err_with_limits(source, MockPluginSession::new(), limits.clone());
    assert!(
        matches!(err, RuntimeError::LoopLimitExceeded { limit: 3 }),
        "expected LoopLimitExceeded with limit 3, got {err:?}"
    );
    let result = run_with_limits(source, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_string_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 5,
        ..Default::default()
    };
    let source = r#"let s = "hello" + " world""#;
    let err = run_expect_runtime_err_with_limits(source, MockPluginSession::new(), limits.clone());
    assert!(
        matches!(err, RuntimeError::StringTooLong { len: 11, limit: 5 }),
        "expected StringTooLong with len 11 and limit 5, got {err:?}"
    );
    let result = run_with_limits(source, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_step_limit_with_ignore() {
    let b64 = compile_dsl_source(
        "let a=1\nlet b=2\nlet c=3\nlet d=4\nlet e=5\nlet f=6",
        &ValidationLimits::default(),
    )
    .expect("compile failed");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    let vm = Vm::new(
        script,
        VmLimits {
            max_steps: 3,
            ..Default::default()
        },
    )
    .expect("vm failed");
    let result = run_vm_policy(&vm, MockPluginSession::new(), &DslErrorPolicy::Ignore);
    assert_eq!(result, good_next());
}

#[test]
fn test_loop_limit_with_deny_with() {
    let b64 = compile_dsl_source(r#"while true { }"#, &ValidationLimits::default()).expect("compile failed");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    let vm = Vm::new(
        script,
        VmLimits {
            max_loop_iterations: 2,
            ..Default::default()
        },
    )
    .expect("vm failed");
    let policy = DslErrorPolicy::DenyWith {
        status: 429,
        body: Some("slow".to_string()),
    };
    let result = run_vm_policy(&vm, MockPluginSession::new(), &policy);
    assert_eq!(result, err_response_body(429, "slow"));
}

// ---------------------------------------------------------------------------
// Collection size limits — max_map_size / max_list_len
// ---------------------------------------------------------------------------

// ① MakeMap literal whose entry count exceeds VmLimits.max_map_size triggers
// RuntimeError::MapTooLarge at OpCode::MakeMap (vm.rs). DslErrorPolicy::Deny
// converts it into ErrResponse { status: 500 }. The validator's compile-time
// cap is the default (512), so this exercises the runtime defense-in-depth path.
#[test]
fn test_map_literal_too_large() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_map_size: 2,
        ..Default::default()
    };
    let source = r#"let m = {"a": 1, "b": 2, "c": 3}"#;
    let err = run_expect_runtime_err_with_limits(source, MockPluginSession::new(), limits.clone());
    assert!(
        matches!(err, RuntimeError::MapTooLarge { size: 3, limit: 2 }),
        "expected MapTooLarge with size 3 and limit 2, got {err:?}"
    );
    let result = run_with_limits(source, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

// ② split() returning more elements than max_list_len triggers RuntimeError::ListTooLong
// before allocating the result list (builtins.rs estimated-length check).
#[test]
fn test_split_result_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_list_len: 3,
        ..Default::default()
    };
    let result = run_with_limits(r#"let xs = split("a,b,c,d,e,f", ",")"#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

// ⑤ Boundary case: MakeMap with exactly max_map_size entries must succeed.
// Guards against off-by-one in the runtime check `count > max_map_size`.
#[test]
fn test_map_literal_exactly_at_limit() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_map_size: 3,
        ..Default::default()
    };
    let result = run_with_limits(r#"let m = {"a": 1, "b": 2, "c": 3}"#, session, limits);
    assert_eq!(result, good_next());
}

#[test]
fn nested_map_construction_preserves_outer_operands_and_last_duplicate() {
    let source = r#"
let outer = {
    "before": "keep",
    "nested": {"duplicate": "first", "items": range(0,3), "duplicate": "last", "empty": {}},
    "after": "tail"
}
if outer["before"] != "keep" { return deny(500, "lost prefix") }
if outer["after"] != "tail" { return deny(500, "lost suffix") }
if outer["nested"]["duplicate"] != "last" { return deny(500, "wrong duplicate") }
if join(outer["nested"]["items"], ",") != "0,1,2" { return deny(500, "lost list") }
if outer["nested"]["empty"]["missing"] != nil { return deny(500, "wrong empty map") }
return next()
"#;
    assert_eq!(run(source, MockPluginSession::new()), good_next());
}

// ⑥ Boundary case: split() returning exactly max_list_len elements must succeed.
#[test]
fn test_split_exactly_at_limit() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_list_len: 3,
        ..Default::default()
    };
    let result = run_with_limits(r#"let xs = split("a,b,c", ",")"#, session, limits);
    assert_eq!(result, good_next());
}

#[test]
fn vm_map_depth_has_exact_boundary_after_validated_bytecode_load() {
    let source = "let mut x = {}\nfor i in range(0,63) { x = {\"a\": x} }";
    let encoded = compile_dsl_source(source, &ValidationLimits::default()).expect("compile failed");
    let script = CompiledScript::deserialize_base64(&encoded).expect("deserialize failed");
    assert!(edgion_dsl::validator::Validator::new(ValidationLimits::default())
        .validate(&script)
        .is_empty());
    let vm = Vm::new(script, VmLimits::default()).expect("vm failed");
    assert_eq!(
        run_vm_policy(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny),
        good_next()
    );

    let over = "let mut x = {}\nfor i in range(0,64) { x = {\"a\": x} }";
    let error = run_expect_runtime_err_with_limits(over, MockPluginSession::new(), VmLimits::default());
    assert!(matches!(error, RuntimeError::MapTooDeep { limit: 64 }), "got {error:?}");
    let response = run_with_limits(over, MockPluginSession::new(), VmLimits::default());
    assert!(matches!(response, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn branching_maps_stop_before_unbounded_growth() {
    let source = "let mut x = {}\nfor i in range(0,10000) { x = {\"a\": x, \"b\": x} }";
    let error = run_expect_runtime_err_with_limits(source, MockPluginSession::new(), VmLimits::default());
    assert!(
        matches!(
            error,
            RuntimeError::ValueTooManyItems { .. } | RuntimeError::ValueWorkLimitExceeded { .. }
        ),
        "got {error:?}"
    );
}

#[test]
fn repeated_wide_map_and_list_copies_consume_value_work_budget() {
    let entries = (0..100).map(|i| format!("\"k{i}\": {i}")).collect::<Vec<_>>().join(",");
    let source = format!("let m = {{{entries}}}\nfor i in range(0,3000) {{ let y = m }}");
    let limits = VmLimits {
        max_steps: 100_000,
        max_loop_iterations: 3000,
        max_stack_depth: 512,
        ..Default::default()
    };
    let error = run_expect_runtime_err_with_limits(&source, MockPluginSession::new(), limits.clone());
    assert!(
        matches!(error, RuntimeError::ValueWorkLimitExceeded { .. }),
        "got {error:?}"
    );

    let source = "let xs = range(0,1000)\nfor i in range(0,3000) { let y = xs }";
    let error = run_expect_runtime_err_with_limits(source, MockPluginSession::new(), limits);
    assert!(
        matches!(error, RuntimeError::ValueWorkLimitExceeded { .. }),
        "got {error:?}"
    );
}

#[test]
fn repeated_long_string_equality_is_charged_before_comparison() {
    let source = format!(
        "let s = \"{}\"\nfor i in range(0,3000) {{ if s == s {{ }} }}",
        "a".repeat(4000)
    );
    let limits = VmLimits {
        max_steps: 100_000,
        max_loop_iterations: 3000,
        ..Default::default()
    };
    let error = run_expect_runtime_err_with_limits(&source, MockPluginSession::new(), limits);
    assert!(
        matches!(error, RuntimeError::ValueWorkLimitExceeded { .. }),
        "got {error:?}"
    );
}

#[test]
fn map_index_preserves_nested_values_and_original_local() {
    let source = r#"
let parent = {"child": {"value": "ok", "items": range(0,3)}}
let child = parent["child"]
if child["value"] != "ok" { return deny(500, "index mismatch") }
if child["items"][2] != "2" { return deny(500, "index mismatch") }
if parent["child"]["value"] != "ok" { return deny(500, "index mismatch") }
if parent["child"]["items"][0] != "0" { return deny(500, "index mismatch") }
if parent["missing"] != nil { return deny(500, "index mismatch") }
if parent[1] != nil { return deny(500, "index mismatch") }
return next()
"#;
    assert_eq!(run(source, MockPluginSession::new()), good_next());
}

#[test]
fn map_index_retains_conservative_work_charge() {
    let entries = (0..100).map(|i| format!("\"k{i}\": {i}")).collect::<Vec<_>>().join(",");
    let source = |iterations| {
        format!(
            "let mut i = 0\nwhile i < {iterations} {{ let child = {{\"child\": {{{entries}}}}}[\"child\"]\ni = i + 1 }}"
        )
    };
    let limits = VmLimits {
        max_steps: 1_000_000,
        max_loop_iterations: 700,
        max_stack_depth: 512,
        ..Default::default()
    };
    // Literal containers avoid GetLocal copy charges. At 700 iterations the
    // legacy IndexGet precharge crosses the budget; omitting it would succeed.
    assert_eq!(
        run_with_limits(&source(650), MockPluginSession::new(), limits.clone()),
        good_next()
    );
    let error = run_expect_runtime_err_with_limits(&source(700), MockPluginSession::new(), limits);
    assert!(
        matches!(error, RuntimeError::ValueWorkLimitExceeded { .. }),
        "{error:?}"
    );
}

#[test]
fn map_index_value_copy_is_charged() {
    let source = "let m = {\"x\": range(0,1000)}\nfor i in range(0,3000) { let y = m[\"x\"] }";
    let limits = VmLimits {
        max_steps: 100_000,
        max_loop_iterations: 3000,
        ..Default::default()
    };
    let error = run_expect_runtime_err_with_limits(source, MockPluginSession::new(), limits);
    assert!(
        matches!(error, RuntimeError::ValueWorkLimitExceeded { .. }),
        "got {error:?}"
    );
}

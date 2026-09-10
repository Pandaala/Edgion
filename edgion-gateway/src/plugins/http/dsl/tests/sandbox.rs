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

// ③ http.call: when the upstream returns more response headers than max_map_size,
// the headers Map is silently truncated to exactly max_map_size (builtins.rs cap).
// Uses wiremock for the upstream so the cap is exercised against real reqwest output.
#[test]
fn test_http_call_headers_truncated() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let async_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    async_rt.block_on(async {
        let server = MockServer::start().await;
        let mut tpl = ResponseTemplate::new(200);
        for i in 0..50 {
            tpl = tpl.insert_header(&*format!("x-h-{}", i), "v");
        }
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(tpl)
            .mount(&server)
            .await;

        let source = format!(
            r#"let r = http.call("GET", "{}/", nil)
               if len(r["headers"]) == 10 {{ return deny(418, "ok") }}
               return deny(500, "wrong header count")"#,
            server.uri()
        );
        let limits = VmLimits {
            max_map_size: 10,
            ..Default::default()
        };
        let vm = make_vm_with_limits(&source, limits);
        let mut log = PluginLog::new("test");
        let mut session = MockPluginSession::new();
        let runtime = DslRuntime::default();
        let result = execute_safe(
            &vm,
            &mut session as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &runtime,
        )
        .await;
        assert_eq!(result, err_response_body(418, "ok"));
    });
}

// ④ http.call: the top-level result Map { status, body, headers } is built in Rust
// (not via OpCode::MakeMap), so max_map_size does NOT truncate it. Even with
// max_map_size = 2 (which heavily truncates the inner headers sub-map), the
// outer map still carries all three fields.
#[test]
fn test_nested_map_boundary() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let async_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    async_rt.block_on(async {
        let server = MockServer::start().await;
        let mut tpl = ResponseTemplate::new(200).set_body_string("hi");
        for i in 0..10 {
            tpl = tpl.insert_header(&*format!("x-h-{}", i), "v");
        }
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(tpl)
            .mount(&server)
            .await;

        let source = format!(
            r#"let r = http.call("GET", "{}/", nil)
               if r["status"] == 200 && r["body"] == "hi" && len(r) == 3 {{
                   return deny(418, "ok")
               }}
               return deny(500, "shape changed")"#,
            server.uri()
        );
        let limits = VmLimits {
            max_map_size: 2,
            ..Default::default()
        };
        let vm = make_vm_with_limits(&source, limits);
        let mut log = PluginLog::new("test");
        let mut session = MockPluginSession::new();
        let runtime = DslRuntime::default();
        let result = execute_safe(
            &vm,
            &mut session as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &runtime,
        )
        .await;
        assert_eq!(result, err_response_body(418, "ok"));
    });
}

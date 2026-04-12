// src/core/gateway/plugins/http/dsl/tests.rs
//! Unit tests for EdgionDSL VM execution semantics.
//!
//! Uses MockPluginSession to validate behaviour without real HTTP connections.
//! MockPluginSession is not Send — all tests use #[test], not #[tokio::test].

use super::lang::bytecode::CompiledScript;
use super::lang::validator::{compile_dsl_source, ValidationLimits};
use super::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
use crate::core::gateway::plugins::runtime::traits::session::{MockPluginSession, PluginSession};
use crate::core::gateway::plugins::runtime::PluginLog;
use crate::types::filters::PluginRunningResult;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_vm(source: &str) -> Vm {
    let b64 =
        compile_dsl_source(source, &ValidationLimits::default()).unwrap_or_else(|e| panic!("compile failed: {:?}", e));
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    Vm::new(script, VmLimits::default()).expect("vm creation failed")
}

fn make_vm_with_limits(source: &str, limits: VmLimits) -> Vm {
    let b64 =
        compile_dsl_source(source, &ValidationLimits::default()).unwrap_or_else(|e| panic!("compile failed: {:?}", e));
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    Vm::new(script, limits).expect("vm creation failed")
}

fn run(source: &str, session: MockPluginSession) -> PluginRunningResult {
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny)
}

fn run_with_limits(source: &str, session: MockPluginSession, limits: VmLimits) -> PluginRunningResult {
    let vm = make_vm_with_limits(source, limits);
    let mut log = PluginLog::new("test");
    let mut s = session;
    execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny)
}

fn good_next() -> PluginRunningResult {
    PluginRunningResult::GoodNext
}

fn err_response_body(status: u16, body: &str) -> PluginRunningResult {
    PluginRunningResult::ErrResponse {
        status,
        body: Some(body.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Smoke test
// ---------------------------------------------------------------------------

#[test]
fn test_smoke_compile_and_run() {
    let session = MockPluginSession::new();
    let result = run("", session);
    assert_eq!(result, good_next());
}

mod request_builtins {
    use super::*;

    #[test]
    fn test_req_header_present() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|h| h == "X-Foo")
            .returning(|_| Some("bar".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Out" && v == "bar")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let v = req.header(\"X-Foo\")\nif v != nil { req.set_header(\"X-Out\", v) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_header_absent() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|h| h == "X-Foo")
            .returning(|_| None);
        let result = run(
            "let v = req.header(\"X-Foo\")\nif v != nil { req.set_header(\"X-Out\", v) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_method() {
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-M" && v == "POST")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.set_header("X-M", req.method)"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_path() {
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/api/users".to_string());
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-P" && v == "/api/users")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.set_header("X-P", req.path)"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_query() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_query_param()
            .withf(|k| k == "page")
            .returning(|_| Some("2".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Q" && v == "2")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let q = req.query(\"page\")\nif q != nil { req.set_header(\"X-Q\", q) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_query_string() {
        let mut session = MockPluginSession::new();
        session.expect_get_query().returning(|| Some("a=1&b=2".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-QS" && v == "a=1&b=2")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let qs = req.query_string\nif qs != nil { req.set_header(\"X-QS\", qs) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_cookie() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_cookie()
            .withf(|k| k == "session")
            .returning(|_| Some("abc".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-C" && v == "abc")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let c = req.cookie(\"session\")\nif c != nil { req.set_header(\"X-C\", c) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_client_ip() {
        let mut session = MockPluginSession::new();
        session.expect_client_addr().return_const("1.2.3.4".to_string());
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-IP" && v == "1.2.3.4")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.set_header("X-IP", req.client_ip)"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_remote_ip() {
        let mut session = MockPluginSession::new();
        session.expect_remote_addr().return_const("10.0.0.1".to_string());
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-RIP" && v == "10.0.0.1")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.set_header("X-RIP", req.remote_ip)"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_host() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|h| h == "Host")
            .returning(|_| Some("example.com".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-H" && v == "example.com")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run("let h = req.host\nif h != nil { req.set_header(\"X-H\", h) }", session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_scheme() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|h| h == "X-Forwarded-Proto")
            .returning(|_| Some("https".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-S" && v == "https")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.set_header("X-S", req.scheme)"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_uri() {
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/api".to_string());
        session.expect_get_query().returning(|| Some("x=1".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-U" && v == "/api?x=1")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.set_header("X-U", req.uri)"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_content_type() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|h| h == "Content-Type")
            .returning(|_| Some("application/json".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-CT" && v == "application/json")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let ct = req.content_type\nif ct != nil { req.set_header(\"X-CT\", ct) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_has_header_true() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|h| h == "X-Auth")
            .returning(|_| Some("token".to_string()));
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-R" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            r#"if req.has_header("X-Auth") { req.set_header("X-R", "yes") }"#,
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_has_header_false() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|h| h == "X-Auth")
            .returning(|_| None);
        let result = run(
            r#"if req.has_header("X-Auth") { req.set_header("X-R", "yes") }"#,
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_header_names_for_in() {
        let mut session = MockPluginSession::new();
        session.expect_request_headers().returning(|| {
            vec![
                ("X-A".to_string(), "v1".to_string()),
                ("X-B".to_string(), "v2".to_string()),
                ("X-C".to_string(), "v3".to_string()),
            ]
        });
        let result = run(r#"for name in req.header_names { log(name) }"#, session);
        assert_eq!(result, good_next());
    }
}

mod request_mutation {
    use super::*;

    #[test]
    fn test_set_header() {
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-A" && v == "v1")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.set_header("X-A", "v1")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_append_header() {
        let mut session = MockPluginSession::new();
        session
            .expect_append_request_header()
            .withf(|h, v| h == "X-A" && v == "v2")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"req.append_header("X-A", "v2")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_remove_header() {
        let mut session = MockPluginSession::new();
        session
            .expect_remove_request_header()
            .withf(|h| h == "X-Del")
            .times(1)
            .returning(|_| Ok(()));
        let result = run(r#"req.remove_header("X-Del")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_set_uri() {
        let mut session = MockPluginSession::new();
        session
            .expect_set_upstream_uri()
            .withf(|u| u == "/new/path")
            .times(1)
            .returning(|_| Ok(()));
        let result = run(r#"req.set_uri("/new/path")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_set_host() {
        let mut session = MockPluginSession::new();
        session
            .expect_set_upstream_host()
            .withf(|h| h == "new.host")
            .times(1)
            .returning(|_| Ok(()));
        let result = run(r#"req.set_host("new.host")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_set_method() {
        let mut session = MockPluginSession::new();
        session
            .expect_set_upstream_method()
            .withf(|m| m == "POST")
            .times(1)
            .returning(|_| Ok(()));
        let result = run(r#"req.set_method("POST")"#, session);
        assert_eq!(result, good_next());
    }
}

mod response_builtins {
    use super::*;

    #[test]
    fn test_resp_status_read() {
        let mut session = MockPluginSession::new();
        session.expect_get_response_status().returning(|| Some(404));
        session
            .expect_set_response_header()
            .withf(|h, v| h == "X-Status" && v == "not_found")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let s = resp.status\nif s != nil { if s == 404 { resp.set_header(\"X-Status\", \"not_found\") } }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_status_nil() {
        let mut session = MockPluginSession::new();
        session.expect_get_response_status().returning(|| None);
        session
            .expect_set_response_header()
            .withf(|h, v| h == "X-Status" && v == "no_status")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let s = resp.status\nif s == nil { resp.set_header(\"X-Status\", \"no_status\") }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_header_read() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_response_header()
            .withf(|h| h == "X-B")
            .returning(|_| Some("hello".to_string()));
        session
            .expect_set_response_header()
            .withf(|h, v| h == "X-Copy" && v == "hello")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let v = resp.header(\"X-B\")\nif v != nil { resp.set_header(\"X-Copy\", v) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_header_nil() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_response_header()
            .withf(|h| h == "X-B")
            .returning(|_| None);
        session
            .expect_set_response_header()
            .withf(|h, v| h == "X-Copy" && v == "none")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "let v = resp.header(\"X-B\")\nif v == nil { resp.set_header(\"X-Copy\", \"none\") }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_has_header_true() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_response_header()
            .withf(|h| h == "X-B")
            .returning(|_| Some("val".to_string()));
        session
            .expect_set_response_header()
            .withf(|h, v| h == "X-R" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "if resp.has_header(\"X-B\") { resp.set_header(\"X-R\", \"yes\") }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_has_header_false() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_response_header()
            .withf(|h| h == "X-B")
            .returning(|_| None);
        let result = run(
            "if resp.has_header(\"X-B\") { resp.set_header(\"X-R\", \"yes\") }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_set_header() {
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_header()
            .withf(|h, v| h == "X-C" && v == "val")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"resp.set_header("X-C", "val")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_append_header() {
        let mut session = MockPluginSession::new();
        session
            .expect_append_response_header()
            .withf(|h, v| h == "Vary" && v == "Origin")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(r#"resp.append_header("Vary", "Origin")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_remove_header() {
        let mut session = MockPluginSession::new();
        session
            .expect_remove_response_header()
            .withf(|h| h == "X-Remove")
            .times(1)
            .returning(|_| Ok(()));
        let result = run(r#"resp.remove_header("X-Remove")"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_upstream_response_filter_phase() {
        use crate::core::gateway::plugins::http::dsl::config::DslConfig;
        use crate::core::gateway::plugins::http::dsl::plugin::DslPlugin;
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_header()
            .withf(|h, v| h == "X-Phase" && v == "resp")
            .times(1)
            .returning(|_, _| Ok(()));

        let config = DslConfig {
            source: Some(r#"resp.set_header("X-Phase", "resp")"#.to_string()),
            name: "test-resp".into(),
            ..Default::default()
        };
        let plugin = DslPlugin::from_config_response(&config).expect("from_config_response failed");
        let mut log = PluginLog::new("test");
        let mut s = session;
        let result = plugin.run_upstream_response_filter(&mut s as &mut dyn PluginSession, &mut log);
        assert_eq!(result, good_next());
    }
}

mod utility_functions {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn assert_output(source: &str, expected: &str) {
        let expected = expected.to_string();
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(move |h, v| h == "X-Out" && v == expected)
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_sha256() {
        assert_output(
            r#"req.set_header("X-Out", sha256("hello"))"#,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        );
    }

    #[test]
    fn test_md5() {
        assert_output(
            r#"req.set_header("X-Out", md5("hello"))"#,
            "5d41402abc4b2a76b9719d911017c592",
        );
    }

    #[test]
    fn test_regex_find_match() {
        assert_output(
            "let v = regex_find(\"foo-123\", \"\\\\d+\")\nif v != nil { req.set_header(\"X-Out\", v) }",
            "123",
        );
    }

    #[test]
    fn test_regex_find_no_match() {
        let session = MockPluginSession::new();
        let result = run(
            "let v = regex_find(\"foo\", \"\\\\d+\")\nif v != nil { req.set_header(\"X-Out\", v) }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_regex_replace_single() {
        assert_output(
            r#"req.set_header("X-Out", regex_replace("abc-def", "-", "_"))"#,
            "abc_def",
        );
    }

    #[test]
    fn test_regex_replace_all() {
        assert_output(r#"req.set_header("X-Out", regex_replace("a-b-c", "-", "/"))"#, "a/b/c");
    }

    // regex_find / regex_replace — dynamic (variable) pattern falls back to CallBuiltin path
    #[test]
    fn test_regex_find_dynamic_pattern() {
        assert_output(
            r#"let p = "\\d+"
let v = regex_find("foo-123", p)
if v != nil { req.set_header("X-Out", v) }"#,
            "123",
        );
    }

    #[test]
    fn test_regex_replace_dynamic_pattern() {
        assert_output(
            r#"let p = "-"
req.set_header("X-Out", regex_replace("a-b-c", p, "/"))"#,
            "a/b/c",
        );
    }

    // Same literal pattern used twice → only one Regex in constant pool
    #[test]
    fn test_regex_literal_dedup() {
        use super::super::lang::bytecode::{CompiledScript, Constant};
        use super::super::lang::validator::{compile_dsl_source, ValidationLimits};
        let source = r#"
let a = regex_find("foo-123", "\\d+")
let b = regex_find("bar-456", "\\d+")
"#;
        let b64 = compile_dsl_source(source, &ValidationLimits::default()).expect("compile ok");
        let script = CompiledScript::deserialize_base64(&b64).expect("deserialize ok");
        let regex_count = script
            .constants
            .iter()
            .filter(|c| matches!(c, Constant::Regex(_)))
            .count();
        assert_eq!(regex_count, 1, "same pattern should deduplicate to one constant");
    }

    // Nil input to regex_find returns Nil (nil-friendly, consistent with Matches)
    #[test]
    fn test_regex_find_nil_input_returns_nil() {
        let mut session = MockPluginSession::new();
        // req.header("Missing") returns None → nil; regex_find on nil must return nil, not match "nil"
        session
            .expect_header_value()
            .withf(|h| h == "Missing")
            .returning(|_| None);
        let result = run(
            r#"let v = regex_find(req.header("Missing"), "nil")
if v != nil { req.set_header("X-Out", "matched") }"#,
            session,
        );
        assert_eq!(result, good_next());
    }

    // Invalid literal pattern → CompileError at compile time
    #[test]
    fn test_regex_find_invalid_literal_pattern_compile_error() {
        let result = compile_dsl_source(r#"let v = regex_find("foo", "[")"#, &ValidationLimits::default());
        assert!(result.is_err(), "invalid regex literal should fail at compile time");
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("invalid regex")),
            "error should mention invalid regex"
        );
    }

    // Literal pattern emits dedicated OpCode::RegexFind / RegexReplace, not CallBuiltin
    #[test]
    fn test_regex_find_literal_emits_dedicated_opcode() {
        use super::super::lang::bytecode::{BuiltinId, CompiledScript, OpCode};
        let b64 = compile_dsl_source(r#"let v = regex_find("foo-123", "\\d+")"#, &ValidationLimits::default())
            .expect("compile ok");
        let script = CompiledScript::deserialize_base64(&b64).expect("deserialize ok");
        assert!(
            script.code.iter().any(|op| matches!(op, OpCode::RegexFind(_))),
            "literal pattern should emit OpCode::RegexFind"
        );
        assert!(
            !script
                .code
                .iter()
                .any(|op| matches!(op, OpCode::CallBuiltin(BuiltinId::RegexFind, _))),
            "literal pattern should NOT emit CallBuiltin(RegexFind)"
        );
    }

    #[test]
    fn test_regex_replace_literal_emits_dedicated_opcode() {
        use super::super::lang::bytecode::{BuiltinId, CompiledScript, OpCode};
        let b64 = compile_dsl_source(
            r#"let v = regex_replace("a-b", "-", "/")"#,
            &ValidationLimits::default(),
        )
        .expect("compile ok");
        let script = CompiledScript::deserialize_base64(&b64).expect("deserialize ok");
        assert!(
            script.code.iter().any(|op| matches!(op, OpCode::RegexReplace(_))),
            "literal pattern should emit OpCode::RegexReplace"
        );
        assert!(
            !script
                .code
                .iter()
                .any(|op| matches!(op, OpCode::CallBuiltin(BuiltinId::RegexReplace, _))),
            "literal pattern should NOT emit CallBuiltin(RegexReplace)"
        );
    }

    // regex_replace with no match returns the original string unchanged (Cow::Borrowed path)
    #[test]
    fn test_regex_replace_no_match_returns_original() {
        assert_output(
            r#"req.set_header("X-Out", regex_replace("hello", "\\d+", "X"))"#,
            "hello",
        );
    }

    // regex_find / regex_replace result exceeding max_string_len → runtime error
    #[test]
    fn test_regex_find_result_too_long() {
        let session = MockPluginSession::new();
        let limits = VmLimits {
            max_string_len: 3,
            ..Default::default()
        };
        // "abcd" matches [a-z]+ fully; result length 4 > limit 3
        let result = run_with_limits(r#"let v = regex_find("abcd", "[a-z]+")"#, session, limits);
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[test]
    fn test_regex_replace_result_too_long() {
        let session = MockPluginSession::new();
        let limits = VmLimits {
            max_string_len: 3,
            ..Default::default()
        };
        // "a-b" → "aXXb" (length 4) > limit 3
        let result = run_with_limits(r#"let v = regex_replace("a-b", "-", "XX")"#, session, limits);
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[test]
    fn test_base64_encode() {
        assert_output(r#"req.set_header("X-Out", base64_encode("hello"))"#, "aGVsbG8=");
    }

    #[test]
    fn test_base64_decode() {
        assert_output(r#"req.set_header("X-Out", base64_decode("aGVsbG8="))"#, "hello");
    }

    #[test]
    fn test_url_encode() {
        assert_output(r#"req.set_header("X-Out", url_encode("a b+c"))"#, "a%20b%2Bc");
    }

    #[test]
    fn test_url_decode() {
        assert_output(r#"req.set_header("X-Out", url_decode("a%20b"))"#, "a b");
    }

    #[test]
    fn test_to_upper() {
        assert_output(r#"req.set_header("X-Out", to_upper("hello"))"#, "HELLO");
    }

    #[test]
    fn test_to_lower() {
        assert_output(r#"req.set_header("X-Out", to_lower("HELLO"))"#, "hello");
    }

    #[test]
    fn test_substr() {
        assert_output(r#"req.set_header("X-Out", substr("hello", 1, 3))"#, "el");
    }

    #[test]
    fn test_len() {
        assert_output(r#"req.set_header("X-Out", to_str(len("abc")))"#, "3");
    }

    #[test]
    fn test_time_now() {
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, _| h == "X-T")
            .times(1)
            .returning(move |_, v| {
                *captured_clone.lock() = Some(v.to_string());
                Ok(())
            });
        let result = run(r#"req.set_header("X-T", to_str(time_now()))"#, session);
        assert_eq!(result, good_next());
        let val = captured.lock().clone().unwrap();
        let ts: i64 = val.parse().expect("time_now should produce a parseable integer");
        assert!(ts > 0, "time_now() should return a positive timestamp, got {}", ts);
    }

    #[test]
    fn test_to_int() {
        assert_output(r#"req.set_header("X-Out", to_str(to_int("42")))"#, "42");
    }

    #[test]
    fn test_to_str() {
        assert_output(r#"req.set_header("X-Out", to_str(42))"#, "42");
    }

    #[test]
    fn test_log_returns_good_next() {
        let session = MockPluginSession::new();
        let result = run(r#"log("hello")"#, session);
        assert_eq!(result, good_next());
    }
}

mod control_flow {
    use super::*;

    #[test]
    fn test_return_deny() {
        let session = MockPluginSession::new();
        let result = run(r#"return deny(403, "blocked")"#, session);
        assert_eq!(result, err_response_body(403, "blocked"));
    }

    #[test]
    fn test_return_deny_nil_body() {
        let session = MockPluginSession::new();
        let result = run(r#"return deny(403, nil)"#, session);
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None
            }
        );
    }

    #[test]
    fn test_return_next_explicit() {
        let session = MockPluginSession::new();
        let result = run(r#"return next()"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_implicit_return_next() {
        let session = MockPluginSession::new();
        let result = run(r#"let x = 1"#, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_if_else_true_branch() {
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X" && v == "1")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "if true { req.set_header(\"X\", \"1\") } else { req.set_header(\"X\", \"2\") }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_if_else_false_branch() {
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X" && v == "2")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(
            "if false { req.set_header(\"X\", \"1\") } else { req.set_header(\"X\", \"2\") }",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_break_stops_loop() {
        let mut session = MockPluginSession::new();
        // i=0,1,2 执行 set_header; i==3 时 break → 3 次调用
        session.expect_set_request_header().times(3).returning(|_, _| Ok(()));
        let result = run(
            "let mut i = 0\nwhile i < 10 {\nif i == 3 { break }\nreq.set_header(\"X-Log\", to_str(i))\ni = i + 1\n}",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_continue_skips_iteration() {
        let mut session = MockPluginSession::new();
        // i=0,2,4 执行; i=1,3 被 continue 跳过 → 3 次调用
        session.expect_set_request_header().times(3).returning(|_, _| Ok(()));
        let result = run(
            "for i in range(0, 5) {\nif i == 1 { continue }\nif i == 3 { continue }\nreq.set_header(\"X-Log\", to_str(i))\n}",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_break_in_for_range() {
        let mut session = MockPluginSession::new();
        // i=0,1,2 → i==3 break → 3 次
        session.expect_set_request_header().times(3).returning(|_, _| Ok(()));
        let result = run(
            "for i in range(0, 10) {\nif i == 3 { break }\nreq.set_header(\"X-Log\", to_str(i))\n}",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_break_in_for_in() {
        let mut session = MockPluginSession::new();
        session.expect_request_headers().returning(|| {
            vec![
                ("X-A".to_string(), "v1".to_string()),
                ("X-B".to_string(), "v2".to_string()),
                ("X-C".to_string(), "v3".to_string()),
                ("X-D".to_string(), "v4".to_string()),
            ]
        });
        // iterates header_names; breaks at idx==2 → set_header called 2 times
        session.expect_set_request_header().times(2).returning(|_, _| Ok(()));
        let result = run(
            "let mut idx = 0\nfor name in req.header_names {\nif idx == 2 { break }\nreq.set_header(\"X-Item\", name)\nidx = idx + 1\n}",
            session,
        );
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_nested_while_break() {
        let mut session = MockPluginSession::new();
        // outer 执行 3 次，inner 每次立即 break → 3 次 set_header
        session.expect_set_request_header().times(3).returning(|_, _| Ok(()));
        let result = run(
            "let mut i = 0\nwhile i < 3 {\nlet mut j = 0\nwhile j < 5 {\nreq.set_header(\"X-Inner\", \"hit\")\nbreak\n}\ni = i + 1\n}",
            session,
        );
        assert_eq!(result, good_next());
    }
}

mod sandbox {
    use super::*;

    #[test]
    fn test_step_limit_exceeded() {
        let session = MockPluginSession::new();
        let limits = VmLimits {
            max_steps: 5,
            ..Default::default()
        };
        let result = run_with_limits(
            "let a=1\nlet b=2\nlet c=3\nlet d=4\nlet e=5\nlet f=6\nlet g=7\nlet h=8\nlet k=9\nlet l=10",
            session,
            limits,
        );
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[test]
    fn test_loop_limit_exceeded() {
        let session = MockPluginSession::new();
        let limits = VmLimits {
            max_loop_iterations: 3,
            ..Default::default()
        };
        let result = run_with_limits(r#"while true { }"#, session, limits);
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[test]
    fn test_string_too_long() {
        let session = MockPluginSession::new();
        let limits = VmLimits {
            max_string_len: 5,
            ..Default::default()
        };
        let result = run_with_limits(r#"let s = "hello" + " world""#, session, limits);
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    #[test]
    fn test_step_limit_with_ignore() {
        use crate::core::gateway::plugins::http::dsl::lang::bytecode::CompiledScript;
        use crate::core::gateway::plugins::http::dsl::lang::validator::{compile_dsl_source, ValidationLimits};
        use crate::core::gateway::plugins::http::dsl::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};

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
        let session = MockPluginSession::new();
        let mut log = PluginLog::new("test");
        let mut s = session;
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Ignore);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_loop_limit_with_deny_with() {
        use crate::core::gateway::plugins::http::dsl::lang::bytecode::CompiledScript;
        use crate::core::gateway::plugins::http::dsl::lang::validator::{compile_dsl_source, ValidationLimits};
        use crate::core::gateway::plugins::http::dsl::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};

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
        let session = MockPluginSession::new();
        let mut log = PluginLog::new("test");
        let mut s = session;
        let policy = DslErrorPolicy::DenyWith {
            status: 429,
            body: Some("slow".to_string()),
        };
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &policy);
        assert_eq!(result, err_response_body(429, "slow"));
    }
}

mod error_policy {
    use super::*;
    use crate::core::gateway::plugins::http::dsl::lang::bytecode::CompiledScript;
    use crate::core::gateway::plugins::http::dsl::lang::validator::{compile_dsl_source, ValidationLimits};
    use crate::core::gateway::plugins::http::dsl::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};

    fn limited_vm() -> Vm {
        let b64 = compile_dsl_source(
            "let a=1\nlet b=2\nlet c=3\nlet d=4\nlet e=5\nlet f=6",
            &ValidationLimits::default(),
        )
        .expect("compile failed");
        let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
        Vm::new(
            script,
            VmLimits {
                max_steps: 3,
                ..Default::default()
            },
        )
        .expect("vm failed")
    }

    #[test]
    fn test_policy_ignore() {
        let vm = limited_vm();
        let session = MockPluginSession::new();
        let mut log = PluginLog::new("test");
        let mut s = session;
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Ignore);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_policy_deny() {
        let vm = limited_vm();
        let session = MockPluginSession::new();
        let mut log = PluginLog::new("test");
        let mut s = session;
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
        assert_eq!(result, err_response_body(500, "DSL execution error"));
    }

    #[test]
    fn test_policy_deny_with() {
        let vm = limited_vm();
        let session = MockPluginSession::new();
        let mut log = PluginLog::new("test");
        let mut s = session;
        let policy = DslErrorPolicy::DenyWith {
            status: 418,
            body: Some("teapot".to_string()),
        };
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &policy);
        assert_eq!(result, err_response_body(418, "teapot"));
    }
}

mod compile_errors {
    fn compile_err(source: &str) {
        use crate::core::gateway::plugins::http::dsl::lang::validator::{compile_dsl_source, ValidationLimits};
        let result = compile_dsl_source(source, &ValidationLimits::default());
        assert!(result.is_err(), "Expected compile error for: {}", source);
    }

    #[test]
    fn test_break_outside_loop() {
        compile_err("break");
    }

    #[test]
    fn test_continue_outside_loop() {
        compile_err("continue");
    }

    #[test]
    fn test_undefined_variable() {
        compile_err("let x = y");
    }

    #[test]
    fn test_assign_immutable() {
        compile_err("let x = 1\nx = 2");
    }

    #[test]
    fn test_invalid_regex() {
        // The `matches` operator validates regex at compile time
        compile_err(r#"if "hello" matches "[unclosed" { }"#);
    }

    #[test]
    fn test_too_many_instructions() {
        let mut src = String::new();
        for i in 0..300 {
            src.push_str(&format!("let x{} = {}\n", i, i));
        }
        compile_err(&src);
    }

    #[test]
    fn test_shadowing_builtin_namespace() {
        compile_err("let req = 1");
    }
}

mod range_and_string_ops {
    use super::*;

    #[test]
    fn test_range_as_expression() {
        let source = r#"
            let r = range(0, 3)
            let total = len(r)
            req.set_header("X-Total", to_str(total))
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Total" && v == "3")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_range_empty() {
        let source = r#"
            let r = range(5, 3)
            req.set_header("X-Len", to_str(len(r)))
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Len" && v == "0")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_range_iterate() {
        let source = r#"
            let r = range(1, 4)
            let mut sum = 0
            for item in r {
                sum = sum + to_int(item)
            }
            req.set_header("X-Sum", to_str(sum))
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Sum" && v == "6")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_trim() {
        let source = r#"
            let s = trim("  hello  ")
            req.set_header("X-Trimmed", s)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Trimmed" && v == "hello")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_trim_empty() {
        let source = r#"
            let s = trim("")
            req.set_header("X-Empty", s)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Empty" && v.is_empty())
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_split_basic() {
        let source = r#"
            let parts = split("a,b,c", ",")
            req.set_header("X-Count", to_str(len(parts)))
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Count" && v == "3")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_split_iterate() {
        let source = r#"
            let parts = split("admin,user,guest", ",")
            let mut found = "no"
            for role in parts {
                if role == "admin" {
                    found = "yes"
                    break
                }
            }
            req.set_header("X-Found", found)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Found" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_split_no_delimiter_found() {
        let source = r#"
            let parts = split("hello", ",")
            req.set_header("X-Count", to_str(len(parts)))
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Count" && v == "1")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_trim_split_combo() {
        let source = r#"
            let roles = "admin, user, guest"
            let parts = split(roles, ",")
            let mut found = "no"
            for role in parts {
                let trimmed = trim(role)
                if trimmed == "user" {
                    found = "yes"
                    break
                }
            }
            req.set_header("X-Found", found)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|h, v| h == "X-Found" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }
}

// ---------------------------------------------------------------------------
// P0: return terminate()
// ---------------------------------------------------------------------------

mod terminate_tests {
    use super::*;

    #[test]
    fn test_return_terminate_basic() {
        let source = r#"return terminate()"#;
        let session = MockPluginSession::new();
        let result = run(source, session);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[test]
    fn test_return_terminate_with_space() {
        let source = "return terminate ()";
        let session = MockPluginSession::new();
        let result = run(source, session);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[test]
    fn test_return_terminate_conditional() {
        let source = r#"
            if req.header("X-Attack") != nil {
                return terminate()
            }
            return next()
        "#;
        // Case 1: header present → terminate
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-Attack")
            .returning(|_| Some("true".to_string()));
        let result = run(source, session);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);

        // Case 2: header absent → next
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-Attack")
            .returning(|_| None);
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_return_terminate_with_log() {
        let source = r#"
            log("terminating request")
            return terminate()
        "#;
        let session = MockPluginSession::new();
        let vm = make_vm(source);
        let mut log = PluginLog::new("test");
        let mut s = session;
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.log.as_ref().unwrap().contains("terminating request"));
    }
}

// ---------------------------------------------------------------------------
// P0: resp.set_status()
// ---------------------------------------------------------------------------

mod resp_set_status_tests {
    use super::*;

    #[test]
    fn test_resp_set_status_basic() {
        let source = r#"
            resp.set_status(503)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_status()
            .withf(|s| *s == 503)
            .times(1)
            .returning(|_| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_set_status_conditional() {
        let source = r#"
            if resp.status == 502 {
                resp.set_status(503)
                resp.set_header("Retry-After", "30")
            }
        "#;
        let mut session = MockPluginSession::new();
        session.expect_get_response_status().returning(|| Some(502));
        session
            .expect_set_response_status()
            .withf(|s| *s == 503)
            .times(1)
            .returning(|_| Ok(()));
        session
            .expect_set_response_header()
            .withf(|n, v| n == "Retry-After" && v == "30")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_set_status_invalid_code() {
        let source = r#"resp.set_status(999)"#;
        let mut session = MockPluginSession::new();
        // Should get a runtime error for invalid status code
        session.expect_set_response_status().times(0);
        let result = run(source, session);
        // With Deny policy, runtime error → 500
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }
}

// ---------------------------------------------------------------------------
// P2 (pre-implemented): return grpc_error()
// ---------------------------------------------------------------------------

mod grpc_error_tests {
    use super::*;

    #[test]
    fn test_return_grpc_error_basic() {
        let source = r#"return grpc_error(7, "permission denied")"#;
        let session = MockPluginSession::new();
        let result = run(source, session);
        assert_eq!(
            result,
            PluginRunningResult::ErrGrpcError {
                grpc_status: 7,
                message: Some("permission denied".to_string()),
            }
        );
    }

    #[test]
    fn test_return_grpc_error_out_of_range_code() {
        let source = r#"return grpc_error(99, "bad code")"#;
        let session = MockPluginSession::new();
        let result = run(source, session);
        // Out of range (0-16) → defaults to 13 (INTERNAL)
        assert_eq!(
            result,
            PluginRunningResult::ErrGrpcError {
                grpc_status: 13,
                message: Some("bad code".to_string()),
            }
        );
    }

    #[test]
    fn test_return_grpc_error_conditional() {
        let source = r#"
            if req.path.starts_with("/admin") {
                return grpc_error(7, "PERMISSION_DENIED: admin access blocked")
            }
            return next()
        "#;
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/admin/users".to_string());
        let result = run(source, session);
        assert_eq!(
            result,
            PluginRunningResult::ErrGrpcError {
                grpc_status: 7,
                message: Some("PERMISSION_DENIED: admin access blocked".to_string()),
            }
        );
    }
}

// ---------------------------------------------------------------------------
// P1: req.protocol, req.grpc_service, req.grpc_method, req.content_length,
//     req.client_cert_cn, req.client_cert_fingerprint
// ---------------------------------------------------------------------------

mod req_metadata_tests {
    use super::*;
    use crate::types::ctx::ClientCertInfo;

    #[test]
    fn test_req_protocol_default() {
        let source = r#"req.set_header("X-Proto", req.protocol)"#;
        let mut session = MockPluginSession::new();
        session.expect_discover_protocol().returning(|| "http".to_string());
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Proto" && v == "http")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_protocol_grpc() {
        let source = r#"
            if req.protocol == "grpc" {
                req.set_header("X-Type", "grpc")
            }
        "#;
        let mut session = MockPluginSession::new();
        session.expect_discover_protocol().returning(|| "grpc".to_string());
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Type" && v == "grpc")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_grpc_service() {
        let source = r#"
            let svc = req.grpc_service
            if svc != nil {
                req.set_header("X-Service", svc)
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_grpc_service()
            .returning(|| Some("mypackage.MyService".to_string()));
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Service" && v == "mypackage.MyService")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_grpc_method() {
        let source = r#"
            let m = req.grpc_method
            if m == nil {
                req.set_header("X-Has-Method", "no")
            }
        "#;
        let mut session = MockPluginSession::new();
        session.expect_grpc_method().returning(|| None);
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Has-Method" && v == "no")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_content_length() {
        let source = r#"
            if req.content_length > 1048576 {
                return deny(413, "too large")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "Content-Length")
            .returning(|_| Some("2097152".to_string()));
        let result = run(source, session);
        assert_eq!(result, err_response_body(413, "too large"));
    }

    #[test]
    fn test_req_content_length_missing() {
        let source = r#"
            if req.content_length == nil {
                req.set_header("X-Has-CL", "no")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "Content-Length")
            .returning(|_| None);
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Has-CL" && v == "no")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_client_cert_cn() {
        let source = r#"
            let cn = req.client_cert_cn
            if cn == "my-service" {
                req.set_header("X-Auth", "cert-ok")
            }
        "#;
        let mut session = MockPluginSession::new();
        session.expect_client_cert_info().returning(|| {
            Some(ClientCertInfo {
                subject: "CN=my-service".to_string(),
                sans: vec![],
                cn: Some("my-service".to_string()),
                fingerprint: "abc123".to_string(),
            })
        });
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Auth" && v == "cert-ok")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_client_cert_fingerprint() {
        let source = r#"req.set_header("X-FP", req.client_cert_fingerprint)"#;
        let mut session = MockPluginSession::new();
        session.expect_client_cert_info().returning(|| {
            Some(ClientCertInfo {
                subject: "CN=svc".to_string(),
                sans: vec![],
                cn: Some("svc".to_string()),
                fingerprint: "sha256:deadbeef".to_string(),
            })
        });
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-FP" && v == "sha256:deadbeef")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_req_client_cert_cn_no_cert() {
        let source = r#"
            if req.client_cert_cn == nil {
                return deny(403, "cert required")
            }
        "#;
        let mut session = MockPluginSession::new();
        session.expect_client_cert_info().returning(|| None);
        let result = run(source, session);
        assert_eq!(result, err_response_body(403, "cert required"));
    }
}

// ---------------------------------------------------------------------------
// P2: resp.header_names(), resp.content_type
// ---------------------------------------------------------------------------

mod resp_p2_tests {
    use super::*;

    #[test]
    fn test_resp_content_type() {
        let source = r#"
            let ct = resp.content_type
            if ct != nil {
                req.set_header("X-CT", ct)
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_get_response_header()
            .withf(|n| n == "Content-Type")
            .returning(|_| Some("application/json".to_string()));
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-CT" && v == "application/json")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_header_names() {
        let source = r#"
            let names = resp.header_names()
            req.set_header("X-Count", to_str(len(names)))
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_response_header_names()
            .returning(|| vec!["content-type".to_string(), "x-custom".to_string()]);
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Count" && v == "2")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_resp_header_names_empty() {
        let source = r#"
            let names = resp.header_names()
            if len(names) == 0 {
                req.set_header("X-Empty", "true")
            }
        "#;
        let mut session = MockPluginSession::new();
        session.expect_response_header_names().returning(|| vec![]);
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Empty" && v == "true")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }
}

// ---------------------------------------------------------------------------
// P3: Utility functions
// ---------------------------------------------------------------------------

mod p3_utility_tests {
    use super::*;

    #[test]
    fn test_hmac_sha256() {
        let source = r#"
            let sig = hmac_sha256("secret-key", "hello")
            req.set_header("X-Sig", sig)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| {
                n == "X-Sig" && v.len() == 64 // SHA256 hex = 64 chars
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_json_get_string() {
        let source = r#"
            let val = json_get("{\"user\":{\"name\":\"alice\"}}", "user.name")
            req.set_header("X-Name", val)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Name" && v == "alice")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_json_get_number() {
        let source = r#"
            let age = json_get("{\"age\": 30}", "age")
            if age == 30 {
                req.set_header("X-Age", "thirty")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Age" && v == "thirty")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_json_get_missing_key() {
        let source = r#"
            let val = json_get("{\"a\": 1}", "b")
            if val == nil {
                req.set_header("X-Found", "no")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Found" && v == "no")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_json_get_invalid_json() {
        let source = r#"
            let val = json_get("not json", "key")
            if val == nil {
                req.set_header("X-Valid", "no")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Valid" && v == "no")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_json_get_array_index() {
        let source = r#"
            let val = json_get("[\"a\",\"b\",\"c\"]", "1")
            req.set_header("X-Val", val)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Val" && v == "b")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_uuid_format() {
        let source = r#"
            let id = uuid()
            if len(id) == 36 {
                req.set_header("X-Has-UUID", "yes")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Has-UUID" && v == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_hex_encode() {
        let source = r#"req.set_header("X-Hex", hex_encode("AB"))"#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Hex" && v == "4142")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_hex_decode() {
        let source = r#"req.set_header("X-Dec", hex_decode("4142"))"#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Dec" && v == "AB")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_hex_decode_invalid() {
        let source = r#"
            let val = hex_decode("not_hex!")
            if val == nil {
                req.set_header("X-Ok", "nil")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Ok" && v == "nil")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_time_format() {
        let source = r#"
            let ts = 1609459200
            let formatted = time_format(ts, "%Y-%m-%d")
            req.set_header("X-Date", formatted)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Date" && v == "2021-01-01")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_ip_in_cidr_v4_match() {
        let source = r#"
            if ip_in_cidr("10.0.1.5", "10.0.0.0/8") {
                req.set_header("X-Net", "internal")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Net" && v == "internal")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_ip_in_cidr_v4_no_match() {
        let source = r#"
            if ip_in_cidr("192.168.1.1", "10.0.0.0/8") {
                req.set_header("X-Net", "internal")
            } else {
                req.set_header("X-Net", "external")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Net" && v == "external")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_ip_in_cidr_v6() {
        let source = r#"
            if ip_in_cidr("::1", "::1/128") {
                req.set_header("X-Net", "loopback")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Net" && v == "loopback")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_ip_in_cidr_invalid_ip() {
        let source = r#"
            if ip_in_cidr("not-an-ip", "10.0.0.0/8") {
                req.set_header("X-Match", "yes")
            } else {
                req.set_header("X-Match", "no")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Match" && v == "no")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_join() {
        let source = r#"
            let parts = split("a,b,c", ",")
            let joined = join(parts, "-")
            req.set_header("X-Joined", joined)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "X-Joined" && v == "a-b-c")
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }

    #[test]
    fn test_log_warn() {
        let source = r#"
            log_warn("something suspicious")
            return next()
        "#;
        let session = MockPluginSession::new();
        let vm = make_vm(source);
        let mut log = PluginLog::new("test");
        let mut s = session;
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
        assert_eq!(result, good_next());
        assert!(log.log.as_ref().unwrap().contains("warn:"));
    }

    #[test]
    fn test_log_error() {
        let source = r#"
            log_error("critical failure")
            return next()
        "#;
        let session = MockPluginSession::new();
        let vm = make_vm(source);
        let mut log = PluginLog::new("test");
        let mut s = session;
        let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
        assert_eq!(result, good_next());
        assert!(log.log.as_ref().unwrap().contains("error:"));
    }

    #[test]
    fn test_hmac_sha256_verify() {
        // Verify a known HMAC value
        let source = r#"
            let sig = hmac_sha256("key", "data")
            req.set_header("X-Sig", sig)
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| {
                // Known HMAC-SHA256("key", "data") value
                n == "X-Sig" && v == "5031fe3d989c6d1537a013fa6e739da23463fdaec3b70137d828e36ace221bd0"
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let result = run(source, session);
        assert_eq!(result, good_next());
    }
}

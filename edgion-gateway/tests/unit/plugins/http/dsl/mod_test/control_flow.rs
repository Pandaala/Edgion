//! Tests for control flow: if/else, while, for, break, continue, return deny/next.

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
    // i=0,1,2 execute set_header; break when i==3 -> 3 calls
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
    // i=0,2,4 execute; i=1,3 are skipped by continue -> 3 calls
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
    // i=0,1,2 -> i==3 break -> 3 iterations
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
    setup_header_mocks(
        &mut session,
        &[("x-a", "v1"), ("x-b", "v2"), ("x-c", "v3"), ("x-d", "v4")],
    );
    // iterates header_names; breaks at idx==2 → set_header called 2 times
    session.expect_set_request_header().times(2).returning(|_, _| Ok(()));
    let result = run(
        "let mut idx = 0\nfor name in req.header_names {\nif idx == 2 { break }\nreq.set_header(\"X-Item\", name)\nidx = idx + 1\n}",
        session,
    );
    assert_eq!(result, good_next());
}

// ---------------------------------------------------------------------------
// Logical operator value semantics (F-019): && / || always yield a Bool, even
// in a value context where the non-short-circuit branch returns the RHS.
// ---------------------------------------------------------------------------

#[test]
fn test_or_coerces_rhs_to_bool() {
    // Left falsy -> falls through to RHS; RHS must be coerced to Bool(is_truthy).
    assert_sets_request_header(r#"req.set_header("X-Or", to_str(false || "ignored"))"#, "X-Or", "true");
}

#[test]
fn test_and_coerces_rhs_to_bool() {
    // Left truthy -> falls through to RHS; RHS must be coerced to Bool(is_truthy).
    assert_sets_request_header(r#"req.set_header("X-And", to_str(true && "ignored"))"#, "X-And", "true");
}

#[test]
fn test_or_nil_operands_yield_false_not_nil() {
    // Both operands falsy -> RHS (Nil) coerced to Bool(false), not leaked as "nil".
    assert_sets_request_header(r#"req.set_header("X-Or", to_str(nil || nil))"#, "X-Or", "false");
}

#[test]
fn test_and_falsy_left_short_circuits_to_false() {
    // Left falsy short-circuits to LoadFalse; RHS is not evaluated.
    assert_sets_request_header(
        r#"req.set_header("X-And", to_str(false && "ignored"))"#,
        "X-And",
        "false",
    );
}

#[test]
fn test_nested_while_break() {
    let mut session = MockPluginSession::new();
    // outer runs 3 times; inner breaks immediately each time -> 3 set_header calls
    session.expect_set_request_header().times(3).returning(|_, _| Ok(()));
    let result = run(
        "let mut i = 0\nwhile i < 3 {\nlet mut j = 0\nwhile j < 5 {\nreq.set_header(\"X-Inner\", \"hit\")\nbreak\n}\ni = i + 1\n}",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn nested_break_preserves_the_outer_loop_iteration_limit() {
    let cases = [
        ("while", "while true { break }"),
        ("for-range", "for inner in range(0, 2) { break }"),
        ("for-in", "for inner in split(\"a,b\", \",\") { break }"),
    ];

    for (name, inner_loop) in cases {
        let source = format!("let mut outer = 0\nwhile outer < 10 {{\n{inner_loop}\nouter = outer + 1\n}}");
        let limits = VmLimits {
            max_loop_iterations: 2,
            max_steps: 10_000,
            ..Default::default()
        };
        let error = run_expect_runtime_err_with_limits(&source, MockPluginSession::new(), limits);

        assert!(
            matches!(error, RuntimeError::LoopLimitExceeded { limit: 2 }),
            "inner {name} break must preserve the outer loop counter, got {error:?}"
        );
    }
}

#[test]
fn while_continue_counts_toward_the_loop_iteration_limit() {
    let limits = VmLimits {
        max_loop_iterations: 2,
        max_steps: 10_000,
        ..Default::default()
    };
    let error = run_expect_runtime_err_with_limits(
        "let mut i = 0\nwhile i < 10 {\ni = i + 1\ncontinue\n}",
        MockPluginSession::new(),
        limits,
    );
    assert!(
        matches!(error, RuntimeError::LoopLimitExceeded { limit: 2 }),
        "got {error:?}"
    );
}

#[test]
fn compiler_temporaries_do_not_shadow_authored_bindings() {
    for (source, expected) in [
        ("let __range_end__ = 99\nlet mut result = 0\nfor i in range(0, 1) { result = __range_end__ }\nreturn deny(403, to_str(result))", "99"),
        ("let __range_end__ = 2\nlet mut result = 0\nfor i in range(__range_end__, 4) { result = result + i }\nreturn deny(403, to_str(result))", "5"),
        ("let __list__ = 10\nlet mut __idx__ = 1\nlet mut total = 0\nfor i in split(\"1,2\", \",\") { for j in split(\"3,4\", \",\") { total = total + __list__ + __idx__\n__idx__ = __idx__ + 1 } }\nfor k in split(\"5\", \",\") { total = total + __list__ + __idx__ }\nreturn deny(403, to_str(total))", "65"),
        ("let __idx__ = 9\nlet mut total = 0\nfor __idx__ in split(\"1,2\", \",\") { total = total + to_int(__idx__) }\nreturn deny(403, to_str(total + __idx__))", "12"),
    ] {
        assert_eq!(run(source, MockPluginSession::new()), err_response_body(403, expected));
    }
}

#[test]
fn compiler_loop_inputs_keep_once_only_evaluation_order() {
    let mut session = MockPluginSession::new();
    let mut sequence = mockall::Sequence::new();
    session.expect_header_value().times(0);
    for (header, value) in [("End", "3"), ("Start", "1"), ("Items", "a,b")] {
        session
            .expect_request_header_map()
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(http::HeaderMap::from_iter([(
                http::HeaderName::from_bytes(header.as_bytes()).unwrap(),
                http::HeaderValue::from_static(value),
            )]));
    }
    let source = r#"
        let mut total = 0
        for i in range(to_int(req.header("Start")), to_int(req.header("End"))) {
            total = total + i
        }
        for item in split(req.header("Items"), ",") { total = total + 1 }
        return deny(403, to_str(total))
    "#;
    assert_eq!(run(source, session), err_response_body(403, "5"));
}

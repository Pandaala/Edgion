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

// ---------------------------------------------------------------------------
// Logical operator value semantics (F-019): && / || always yield a Bool, even
// in a value context where the non-short-circuit branch returns the RHS.
// ---------------------------------------------------------------------------

#[test]
fn test_or_coerces_rhs_to_bool() {
    // Left falsy -> falls through to RHS; RHS must be coerced to Bool(is_truthy).
    // Before the fix this leaked the raw Str "ignored".
    assert_sets_request_header(r#"req.set_header("X-Or", to_str(false || "ignored"))"#, "X-Or", "true");
}

#[test]
fn test_and_coerces_rhs_to_bool() {
    // Left truthy -> falls through to RHS; RHS must be coerced to Bool(is_truthy).
    // Before the fix this leaked the raw Str "ignored".
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

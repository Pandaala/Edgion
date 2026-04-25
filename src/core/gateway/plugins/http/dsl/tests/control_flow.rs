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

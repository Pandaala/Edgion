//! Tests for range(), trim(), split() and their combinations.

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

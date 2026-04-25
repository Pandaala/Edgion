//! Tests for P2 response builtins: resp.content_type, resp.header_names().

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

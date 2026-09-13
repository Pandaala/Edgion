//! Tests for request mutation: set/append/remove header, set URI/host/method.

use super::*;

#[test]
fn test_set_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-A" && v == "v1")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"let result = req.set_header("X-A", "v1")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.append_header("X-A", "v2")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.remove_header("X-Del")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.set_uri("/new/path")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.set_host("new.host")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.set_method("POST")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[tokio::test]
async fn test_request_method_case_preserved_through_real_adapter() {
    use crate::plugins::runtime::session_adapter::{record_outcome_tests::test_session, PingoraSessionAdapter};
    use crate::EdgionHttpContext;
    use pingora_core::protocols::http::v1::client::http_req_header_to_wire;

    // Exercise both inline and allocated extension methods in the locked http parser.
    for method in ["POST", "post", "Post", "X-Custom", "X-Long-Custom-Method"] {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let literal = serde_json::to_string(method).unwrap();
        let vm = make_vm(&format!(
            r#"let result = req.set_method({literal})
               if result != nil || req.method != {literal} {{ return deny(403, nil) }}"#
        ));
        let result = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            vm.execute(&mut adapter, &mut PluginLog::new("test"), &DslRuntime::default())
                .await
        };
        assert_eq!(result.unwrap(), good_next(), "method: {method}");
        assert_eq!(session.req_header().method.as_str(), method);
        assert_eq!(session.req_header().method == http::Method::POST, method == "POST");
        let wire = http_req_header_to_wire(session.req_header()).expect("valid request header");
        assert!(wire.starts_with(format!("{method} /dye-test HTTP/1.1\r\n").as_bytes()));
    }
}

#[tokio::test]
async fn test_request_method_invalid_input_preserves_method_and_raises_api_error() {
    use crate::plugins::runtime::session_adapter::{record_outcome_tests::test_session, PingoraSessionAdapter};
    use crate::EdgionHttpContext;

    for method in ["", "BAD METHOD", "GET/POST", "POST\r\n", "GÉT"] {
        let mut session = test_session().await;
        session.req_header_mut().set_method("X-Original".parse().unwrap());
        let mut ctx = EdgionHttpContext::default();
        let literal = serde_json::to_string(method).unwrap();
        let vm = make_vm(&format!("req.set_method({literal})"));
        let error = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            vm.execute(&mut adapter, &mut PluginLog::new("test"), &DslRuntime::default())
                .await
                .expect_err("invalid method must fail")
        };
        assert!(matches!(error, RuntimeError::ApiError { function, .. } if function == "req.set_method"));
        assert_eq!(session.req_header().method.as_str(), "X-Original");
    }
}

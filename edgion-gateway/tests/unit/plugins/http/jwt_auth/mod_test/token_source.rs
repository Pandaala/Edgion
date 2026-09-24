//! Token extraction from header / query / cookie.

use super::common::*;
use serde_json::json;

#[tokio::test]
async fn test_token_from_query_param() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.query = "token".to_string();
            c.allow_token_in_query = true;
        }),
    );

    let claims = json!({
        "sub": "query-user",
        "exp": now_ts() + 3600
    });
    let token = generate_hs256_token(claims, TEST_SECRET);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    let token_clone = token.clone();
    mock.expect_get_query_param().returning(move |name| {
        if name == "token" {
            Some(token_clone.clone())
        } else {
            None
        }
    });
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);
    assert!(log.contains("OK group="), "log should contain OK group=, got: {log:?}");
}

#[tokio::test]
async fn test_query_token_rejected_when_not_allowed_by_default() {
    // Default config has allow_token_in_query = false, so a token carried only in
    // the query parameter must be ignored and the request treated as unauthenticated
    // (RFC 6750 §2.3/§5.3). No anonymous fallback is configured, so this 401s.
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);

    let claims = json!({
        "sub": "query-user",
        "exp": now_ts() + 3600
    });
    let token = generate_hs256_token(claims, TEST_SECRET);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    let token_clone = token.clone();
    mock.expect_get_query_param()
        .returning(move |name| if name == "jwt" { Some(token_clone.clone()) } else { None });
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
    assert!(
        log.contains("No token"),
        "query token must be ignored by default, got: {log:?}"
    );
}

#[tokio::test]
async fn test_token_from_cookie() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.cookie = "jwt_cookie".to_string();
        }),
    );

    let claims = json!({
        "sub": "cookie-user",
        "exp": now_ts() + 3600
    });
    let token = generate_hs256_token(claims, TEST_SECRET);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().returning(|_| None);
    let token_clone = token.clone();
    mock.expect_get_cookie().returning(move |name| {
        if name == "jwt_cookie" {
            Some(token_clone.clone())
        } else {
            None
        }
    });
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);
    assert!(log.contains("OK group="), "log should contain OK group=, got: {log:?}");
}

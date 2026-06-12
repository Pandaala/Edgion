//! Algorithm validation: happy path, mismatch rejection, malformed token.

use super::common::*;
use jsonwebtoken::Algorithm;
use serde_json::json;

#[tokio::test]
async fn test_valid_hs256_token_succeeds() {
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);
    let claims = json!({
        "sub": "user123",
        "exp": now_ts() + 3600
    });
    let token = generate_hs256_token(claims, TEST_SECRET);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    let token_clone = token.clone();
    mock.expect_header_value()
        .returning(move |_| Some(format!("Bearer {}", token_clone)));
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);
    assert!(log.contains("OK group="), "log should contain OK group=, got: {log:?}");
}

#[tokio::test]
async fn test_algorithm_mismatch_rejected() {
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);
    let claims = json!({
        "sub": "user123",
        "exp": now_ts() + 3600
    });
    let token = generate_token_with_alg(claims, TEST_SECRET, Algorithm::HS384);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    let token_clone = token.clone();
    mock.expect_header_value()
        .returning(move |_| Some(format!("Bearer {}", token_clone)));
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_invalid_token_returns_401() {
    let auth = jwt_auth_default(None);
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    mock.expect_header_value()
        .returning(|_| Some("Bearer invalid.token.here".to_string()));
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
}

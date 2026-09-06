//! Anonymous access fallback and rejection-on-bad-token semantics.

use super::common::*;

#[tokio::test]
async fn test_no_token_returns_401() {
    let auth = jwt_auth_default(None);
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("No token"));
}

#[tokio::test]
async fn test_no_token_with_anonymous() {
    let auth = jwt_auth_default(Some("anon-user".to_string()));
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);
    assert!(log.contains("Anon=anon-user"));
}

#[tokio::test]
async fn test_no_token_with_anonymous_allows_access() {
    let auth = jwt_auth_default(Some("anon-user".to_string()));
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);
    assert!(log.contains("Anon=anon-user"));
}

#[tokio::test]
async fn test_invalid_token_with_anonymous_must_reject() {
    let auth = jwt_auth_default(Some("anon-user".to_string()));
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[("authorization", "Bearer invalid.token.here")]);
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
}

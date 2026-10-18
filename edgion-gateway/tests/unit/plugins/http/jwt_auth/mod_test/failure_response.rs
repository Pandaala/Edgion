//! 401 response shape (WWW-Authenticate header) and auth_failure_delay timing.

use bytes::Bytes;
use mockall::Sequence;
use tokio::time::{advance, Duration, Instant};

use super::common::*;

const FAILURE_BODY: &[u8] = b"401 Unauthorized - Invalid or missing JWT";

fn expect_auth_response(mock: &mut MockPluginSession, challenge: &'static str) {
    let mut sequence = Sequence::new();
    mock.expect_write_response_header()
        .withf(move |response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 401
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    == Some("text/plain")
                && response
                    .headers
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok())
                    == Some(challenge)
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    mock.expect_write_response_body()
        .withf(|body, end_of_stream| {
            *end_of_stream && body.as_deref() == Some(Bytes::from_static(FAILURE_BODY).as_ref())
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    mock.expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});
}

#[tokio::test]
async fn missing_token_returns_exact_bare_bearer_challenge() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|config| config.realm = "my-api".to_string()),
    );
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");
    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
    expect_auth_response(&mut mock, "Bearer realm=\"my-api\"");

    assert_eq!(
        auth.run_request(&mut mock, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("No token"), "got: {log:?}");
    assert!(!log.contains("Deny"), "got: {log:?}");
}

#[tokio::test]
async fn invalid_token_returns_exact_invalid_token_challenge() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|config| config.realm = "my-api".to_string()),
    );
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");
    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[("authorization", "Bearer not-a-jwt")]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
    expect_auth_response(
        &mut mock,
        "Bearer realm=\"my-api\", error=\"invalid_token\", error_description=\"Unauthorized - Invalid or missing JWT\"",
    );

    assert_eq!(
        auth.run_request(&mut mock, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("Deny malformed token"), "got: {log:?}");
    assert!(!serde_json::to_string(&log).unwrap().contains("not-a-jwt"));
}

#[tokio::test(start_paused = true)]
async fn auth_failure_delay_is_exact_under_paused_time() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|config| config.auth_failure_delay_ms = 100),
    );
    let mut mock = MockPluginSession::new();
    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
    expect_auth_response(&mut mock, "Bearer realm=\"jwt\"");

    let start = Instant::now();
    let task = tokio::spawn(async move {
        let mut log = PluginLog::new("JwtAuth");
        let result = auth.run_request(&mut mock, &mut log).await;
        (result, log)
    });
    tokio::task::yield_now().await;
    assert!(!task.is_finished());

    advance(Duration::from_millis(99)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());

    advance(Duration::from_millis(1)).await;
    let (result, log) = task.await.unwrap();
    assert_eq!(Instant::now().duration_since(start), Duration::from_millis(100));
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("No token"), "got: {log:?}");
}

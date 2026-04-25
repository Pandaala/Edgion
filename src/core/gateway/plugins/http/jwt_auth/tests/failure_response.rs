//! 401 response shape (WWW-Authenticate header) and auth_failure_delay timing.

use super::common::*;
use parking_lot::Mutex;
use std::sync::Arc;

#[tokio::test]
async fn test_401_includes_www_authenticate_header() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.realm = "my-api".to_string();
        }),
    );

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_method().returning(|| "GET".to_string());
    mock.expect_header_value().returning(|_| None);
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);

    let headers_captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let headers_clone = headers_captured.clone();
    mock.expect_write_response_header().returning(move |resp, _| {
        if let Some(val) = resp.headers.get("WWW-Authenticate") {
            headers_clone
                .lock()
                .push(("WWW-Authenticate".to_string(), val.to_str().unwrap_or("").to_string()));
        }
        Ok(())
    });
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);

    let headers = headers_captured.lock();
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "WWW-Authenticate" && v.contains("Bearer") && v.contains("my-api")),
        "Expected WWW-Authenticate header with realm, got: {:?}",
        *headers
    );
}

#[tokio::test]
async fn test_auth_failure_delay() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.auth_failure_delay_ms = 100;
        }),
    );

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_method().returning(|| "GET".to_string());
    mock.expect_header_value().returning(|_| None);
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let start = std::time::Instant::now();
    let r = auth.run_request(&mut mock, &mut log).await;
    let elapsed = start.elapsed();

    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
    assert!(elapsed.as_millis() >= 90, "Expected delay of ~100ms, got {:?}", elapsed);
}

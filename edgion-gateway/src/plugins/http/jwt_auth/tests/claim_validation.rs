//! Issuer / audience / maximum_expiration validation.

use super::common::*;
use serde_json::json;

#[tokio::test]
async fn test_issuer_validation_pass() {
    // With issuer_groups, the iss is routed to the group. A token with the
    // correct TEST_ISS is accepted.
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);

    let claims = json!({
        "sub": "user123",
        "iss": TEST_ISS,
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
}

#[tokio::test]
async fn test_issuer_validation_fail() {
    // A token with an unknown iss is rejected with "Deny unknown iss".
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);

    let claims = json!({
        "sub": "user123",
        "iss": "https://wrong-issuer.com",
        "exp": now_ts() + 3600
    });
    // Generate token with wrong iss explicitly (bypass the helper merging TEST_ISS).
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let token = {
        let header = Header::new(Algorithm::HS256);
        encode(&header, &claims, &EncodingKey::from_secret(TEST_SECRET.as_bytes())).unwrap()
    };

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
    assert!(log.contains("Deny unknown iss"), "got: {log:?}");
}

#[tokio::test]
async fn test_audience_validation_pass() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            // audiences on config is snapshot-merged into the group during
            // jwt_auth_with_credential construction
            c.audiences = Some(vec!["my-api".to_string()]);
        }),
    );

    let claims = json!({
        "sub": "user123",
        "aud": "my-api",
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
}

#[tokio::test]
async fn test_audience_validation_fail() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.audiences = Some(vec!["my-api".to_string()]);
        }),
    );

    let claims = json!({
        "sub": "user123",
        "aud": "wrong-api",
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
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_maximum_expiration_pass() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.maximum_expiration = 7200;
        }),
    );

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
}

#[tokio::test]
async fn test_maximum_expiration_exceeded() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.maximum_expiration = 1800;
        }),
    );

    let claims = json!({
        "sub": "user123",
        "exp": now_ts() + 7200
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
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
}

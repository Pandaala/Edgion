//! Issuer / audience / maximum_expiration validation.

use super::common::*;
use serde_json::json;

async fn run_and_assert_accept(auth: JwtAuth, token: &str) {
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");
    mock.expect_get_method().return_const("GET".to_string());
    let token = token.to_string();
    setup_header_mocks(&mut mock, &[("authorization", &format!("Bearer {token}"))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    assert_eq!(
        auth.run_request(&mut mock, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

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
    setup_header_mocks(&mut mock, &[("authorization", &format!("Bearer {}", token_clone))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
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
    setup_header_mocks(&mut mock, &[("authorization", &format!("Bearer {}", token_clone))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
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
    setup_header_mocks(&mut mock, &[("authorization", &format!("Bearer {}", token_clone))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
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
    setup_header_mocks(&mut mock, &[("authorization", &format!("Bearer {}", token_clone))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn configured_audience_requires_the_aud_claim() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|config| config.audiences = Some(vec!["my-api".to_string()])),
    );
    let token = generate_hs256_token(
        json!({
            "sub": "user123",
            "exp": now_ts() + 3600
        }),
        TEST_SECRET,
    );

    let log = run_and_assert_reject(auth, &token).await;
    assert!(log.contains("Deny invalid token"), "got: {log:?}");
    assert_no_token_leak(&log, &token);
    let serialized = serde_json::to_string(&log).unwrap();
    for forbidden in [TEST_ISS, TEST_KID, "test-group", "HS256"] {
        assert!(
            !serialized.contains(forbidden),
            "failure log leaked `{forbidden}`: {serialized}"
        );
    }
}

#[tokio::test]
async fn absent_or_explicitly_empty_audiences_do_not_require_aud() {
    let token = generate_hs256_token(
        json!({
            "sub": "user123",
            "exp": now_ts() + 3600
        }),
        TEST_SECRET,
    );

    run_and_assert_accept(jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None), &token).await;
    run_and_assert_accept(
        jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|config| config.audiences = Some(Vec::new())),
        ),
        &token,
    )
    .await;
}

#[tokio::test]
async fn nbf_is_enforced_with_configured_leeway() {
    let within_leeway = generate_hs256_token(
        json!({
            "sub": "user123",
            "nbf": now_ts() + 10,
            "exp": now_ts() + 3600
        }),
        TEST_SECRET,
    );
    run_and_assert_accept(
        jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|config| config.lifetime_grace_period = 30),
        ),
        &within_leeway,
    )
    .await;

    let beyond_leeway = generate_hs256_token(
        json!({
            "sub": "user123",
            "nbf": now_ts() + 60,
            "exp": now_ts() + 3600
        }),
        TEST_SECRET,
    );
    let log = run_and_assert_reject(
        jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|config| config.lifetime_grace_period = 30),
        ),
        &beyond_leeway,
    )
    .await;
    assert!(log.contains("Deny immature token"), "got: {log:?}");
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
    setup_header_mocks(&mut mock, &[("authorization", &format!("Bearer {}", token_clone))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
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
    setup_header_mocks(&mut mock, &[("authorization", &format!("Bearer {}", token_clone))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn fractional_expiration_cannot_bypass_maximum_expiration() {
    let within_limit = generate_hs256_token(
        json!({
            "sub": "user123",
            "exp": now_ts() as f64 + 100.49
        }),
        TEST_SECRET,
    );
    run_and_assert_accept(
        jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|config| config.maximum_expiration = 200),
        ),
        &within_limit,
    )
    .await;

    let beyond_limit = generate_hs256_token(
        json!({
            "sub": "user123",
            "exp": now_ts() as f64 + 7200.5
        }),
        TEST_SECRET,
    );
    let log = run_and_assert_reject(
        jwt_auth_with_credential(
            TEST_SECRET,
            JwtAlgorithm::HS256,
            Some(|config| config.maximum_expiration = 1800),
        ),
        &beyond_limit,
    )
    .await;
    assert!(log.contains("Deny exp too far"), "got: {log:?}");
}

#[tokio::test]
async fn prepared_generations_are_key_isolated() {
    const SECRET_ONE: &str = "generation-one-secret-at-least-32-bytes-long";
    const SECRET_TWO: &str = "generation-two-secret-at-least-32-bytes-long";
    let claims = json!({
        "sub": "user123",
        "exp": now_ts() + 3600
    });
    let token_one = generate_hs256_token(claims.clone(), SECRET_ONE);
    let token_two = generate_hs256_token(claims, SECRET_TWO);

    run_and_assert_accept(
        jwt_auth_with_credential(SECRET_ONE, JwtAlgorithm::HS256, None),
        &token_one,
    )
    .await;
    run_and_assert_accept(
        jwt_auth_with_credential(SECRET_TWO, JwtAlgorithm::HS256, None),
        &token_two,
    )
    .await;

    let old_with_new = run_and_assert_reject(
        jwt_auth_with_credential(SECRET_ONE, JwtAlgorithm::HS256, None),
        &token_two,
    )
    .await;
    let new_with_old = run_and_assert_reject(
        jwt_auth_with_credential(SECRET_TWO, JwtAlgorithm::HS256, None),
        &token_one,
    )
    .await;
    assert!(old_with_new.contains("Deny invalid signature"));
    assert!(new_with_old.contains("Deny invalid signature"));
}

//! Claims-to-headers, store_claims_in_ctx, username extraction.

use super::common::*;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap as StdHashMap;
use std::sync::Arc;

#[tokio::test]
async fn test_claims_to_headers() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            let mut mapping = StdHashMap::new();
            mapping.insert("sub".to_string(), "X-User-ID".to_string());
            mapping.insert("email".to_string(), "X-User-Email".to_string());
            mapping.insert("roles".to_string(), "X-User-Roles".to_string());
            c.claims_to_headers = Some(mapping);
        }),
    );

    let claims = json!({
        "sub": "user123",
        "email": "user@example.com",
        "roles": ["admin", "user"],
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

    let headers_set: Arc<Mutex<StdHashMap<String, String>>> = Arc::new(Mutex::new(StdHashMap::new()));
    let headers_clone = headers_set.clone();
    mock.expect_set_request_header_value().returning(move |name, value| {
        assert!(value.is_sensitive());
        headers_clone
            .lock()
            .insert(name.to_string(), value.to_str().unwrap().to_string());
        Ok(())
    });

    mock.expect_remove_request_header()
        .withf(|name| name.starts_with("x-user-"))
        .times(3)
        .returning(|_| Ok(()));
    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);

    let headers = headers_set.lock();
    assert_eq!(headers.get("x-user-id"), Some(&"user123".to_string()));
    assert_eq!(headers.get("x-user-email"), Some(&"user@example.com".to_string()));
    assert_eq!(headers.get("x-user-roles"), Some(&"admin,user".to_string()));
}

#[tokio::test]
async fn test_store_claims_in_ctx() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.store_claims_in_ctx = true;
        }),
    );

    let claims = json!({
        "sub": "user123",
        "email": "user@example.com",
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

    let ctx_var_captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let ctx_clone = ctx_var_captured.clone();
    mock.expect_set_ctx_var().returning(move |key, value| {
        if key == "jwt_claims" {
            *ctx_clone.lock() = Some(value.to_string());
        }
        Ok(())
    });

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);

    let ctx_val = ctx_var_captured.lock();
    assert!(ctx_val.is_some(), "jwt_claims should be set in context");
    let claims_json = ctx_val.as_ref().unwrap();
    assert!(claims_json.contains("user123"));
    assert!(claims_json.contains("user@example.com"));
}

#[tokio::test]
async fn test_username_from_sub() {
    // Username is extracted from the "sub" claim.
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);

    let claims = json!({
        "sub": "sub-value",
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
    // OK log includes group, iss, and u=<sub>
    assert!(log.contains("OK group="), "log should contain OK group=, got: {log:?}");
    assert!(
        log.contains("u=sub-value"),
        "log should contain u=sub-value, got: {log:?}"
    );
}

#[tokio::test]
async fn test_username_missing_sub_uses_placeholder() {
    // When no sub claim is present, username is "(missing sub)".
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);

    let claims = json!({
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
    assert!(log.contains("OK group="), "log should contain OK group=, got: {log:?}");
}

//! kind tag classification and no-token-leak guarantee in PluginLog.

use super::common::*;
use serde_json::json;

#[tokio::test]
async fn test_kind_tag_expired() {
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);
    let claims = json!({ "sub": "user123", "iat": now_ts() - 7200, "exp": now_ts() - 3600 });
    let token = generate_hs256_token(claims, TEST_SECRET);

    let log = run_and_assert_reject(auth, &token).await;
    assert!(log.contains("Deny token expired"));
    assert_no_token_leak(&log, &token);
}

#[tokio::test]
async fn test_kind_tag_invalid_sig() {
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);
    let claims = json!({ "sub": "user123", "exp": now_ts() + 3600 });
    let token = generate_hs256_token(claims, "wrong-secret-for-signature-verification-test-padding");

    let log = run_and_assert_reject(auth, &token).await;
    assert!(log.contains("Deny invalid signature"));
    assert_no_token_leak(&log, &token);
}

#[tokio::test]
async fn test_kind_tag_malformed() {
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);
    let token = "not.a.valid-jwt".to_string();

    let log = run_and_assert_reject(auth, &token).await;
    assert!(log.contains("Deny malformed"));
    assert_no_token_leak(&log, &token);
}

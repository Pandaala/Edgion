//! base64_secret config decoding.

use super::common::*;
use base64::Engine;
use serde_json::json;

#[tokio::test]
async fn test_base64_secret_decoding() {
    let raw_secret = "my-raw-secret-key-for-testing!!";
    let base64_secret = base64::engine::general_purpose::STANDARD.encode(raw_secret);

    let auth = jwt_auth_with_credential(
        &base64_secret,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.base64_secret = true;
        }),
    );

    let claims = json!({
        "sub": "user123",
        "exp": now_ts() + 3600
    });
    let token = generate_hs256_token(claims, raw_secret);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_method().returning(|| "GET".to_string());
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

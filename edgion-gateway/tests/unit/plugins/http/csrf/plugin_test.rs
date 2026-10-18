use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;

fn create_csrf_config() -> CsrfConfig {
    CsrfConfig {
        name: "X-CSRF-Token".to_string(),
        secret_ref: None,
        resolved_key: Some("test-secret-key-32-bytes-long!".to_string()),
        expires: 7200,
        cookie_secure: true,
    }
}

#[test]
fn plugin_name_is_static_across_configurations() {
    let first = Csrf::new(&create_csrf_config());
    let mut second_config = create_csrf_config();
    second_config.name = "X-Other-CSRF-Token".to_string();
    let second = Csrf::new(&second_config);

    assert_eq!(first.name(), "Csrf");
    assert_eq!(second.name(), "Csrf");
}

#[test]
fn construction_pins_only_the_runtime_generation_fields() {
    let mut config = create_csrf_config();
    let csrf = Csrf::new(&config);

    config.name = "X-Replaced-CSRF-Token".to_string();
    config.expires = 1;
    config.cookie_secure = false;
    config.resolved_key = Some("replacement-key".to_string());

    assert_eq!(csrf.token_name, "X-CSRF-Token");
    assert_eq!(csrf.expires, 7200);
    assert!(csrf.cookie_secure);
    assert_eq!(csrf.key, "test-secret-key-32-bytes-long!");
}

#[tokio::test]
async fn test_safe_method_sets_cookie() {
    let config = create_csrf_config();
    let csrf = Csrf::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Csrf");

    mock_session.expect_get_method().return_const("GET".to_string());
    mock_session
        .expect_append_response_header()
        .withf(|name, value| {
            name == "Set-Cookie"
                && value.starts_with("X-CSRF-Token=")
                && value.contains("; Max-Age=7200")
                && value.contains("; Path=/")
                && value.contains("; SameSite=Lax")
                && !value.contains("HttpOnly")
        })
        .returning(|_, _| Ok(()));

    let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(plugin_log.contains("Token set"));
}

#[tokio::test]
async fn test_cookie_secure_true_emits_secure_attribute() {
    let config = create_csrf_config(); // cookie_secure = true
    let csrf = Csrf::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Csrf");

    mock_session.expect_get_method().return_const("GET".to_string());
    mock_session
        .expect_append_response_header()
        .withf(|name, value| name == "Set-Cookie" && value.contains("; Secure"))
        .returning(|_, _| Ok(()));

    let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(plugin_log.contains("Token set"));
}

#[tokio::test]
async fn test_cookie_secure_false_omits_secure_attribute() {
    let mut config = create_csrf_config();
    config.cookie_secure = false;
    let csrf = Csrf::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Csrf");

    mock_session.expect_get_method().return_const("GET".to_string());
    mock_session
        .expect_append_response_header()
        .withf(|name, value| name == "Set-Cookie" && !value.contains("; Secure"))
        .returning(|_, _| Ok(()));

    let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(plugin_log.contains("Token set"));
}

#[tokio::test]
async fn test_missing_header_token() {
    let config = create_csrf_config();
    let csrf = Csrf::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Csrf");

    mock_session.expect_get_method().return_const("POST".to_string());
    setup_header_mocks(&mut mock_session, &[]);

    let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 401,
            body: Some(Bytes::from_static(br#"{"error_msg":"no csrf token in headers"}"#)),
        }
    );
    assert!(plugin_log.contains("Deny missing token"));
}

#[tokio::test]
async fn test_missing_cookie_token() {
    let config = create_csrf_config();
    let csrf = Csrf::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Csrf");

    mock_session.expect_get_method().return_const("POST".to_string());
    setup_header_mocks(&mut mock_session, &[("X-CSRF-Token", "some-token")]);
    mock_session.expect_get_cookie().times(0);

    let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 401,
            body: Some(Bytes::from_static(br#"{"error_msg":"no csrf cookie"}"#)),
        }
    );
    assert!(plugin_log.contains("Deny missing token"));
}

#[tokio::test]
async fn test_token_mismatch() {
    let config = create_csrf_config();
    let csrf = Csrf::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Csrf");

    mock_session.expect_get_method().return_const("POST".to_string());
    setup_header_mocks(
        &mut mock_session,
        &[("X-CSRF-Token", "token1"), ("Cookie", "X-CSRF-Token=token2")],
    );
    mock_session.expect_get_cookie().times(0);

    let result = csrf.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 401,
            body: Some(Bytes::from_static(br#"{"error_msg":"csrf token mismatch"}"#)),
        }
    );
    assert!(plugin_log.contains("Deny bad token"));
}

#[tokio::test]
async fn malformed_matching_token_preserves_the_signature_failure_response() {
    let csrf = Csrf::new(&create_csrf_config());
    let mut mock_session = MockPluginSession::new();
    mock_session.expect_get_method().return_const("POST".to_string());
    setup_header_mocks(
        &mut mock_session,
        &[
            ("X-CSRF-Token", "not-a-valid-csrf-token"),
            ("Cookie", "X-CSRF-Token=not-a-valid-csrf-token"),
        ],
    );
    mock_session.expect_get_cookie().times(0);

    let mut plugin_log = PluginLog::new("Csrf");
    assert_eq!(
        csrf.run_request(&mut mock_session, &mut plugin_log).await,
        PluginRunningResult::ErrResponse {
            status: 401,
            body: Some(Bytes::from_static(
                br#"{"error_msg":"Failed to verify the csrf token signature"}"#
            )),
        }
    );
    assert!(plugin_log.contains("Deny bad token"));
}

#[tokio::test]
async fn padded_real_token_uses_the_canonical_cookie_accessor() {
    let config = create_csrf_config();
    let csrf = Csrf::new(&config);
    let token = CsrfToken::generate(&csrf.key).encode().unwrap();
    assert!(token.ends_with("=="));

    let mut mock_session = MockPluginSession::new();
    mock_session.expect_get_method().return_const("POST".to_string());
    let cookie = format!("X-CSRF-Token={token}");
    setup_header_mocks(
        &mut mock_session,
        &[("x-csrf-token", token.as_str()), ("Cookie", cookie.as_str())],
    );
    mock_session.expect_get_cookie().times(0);

    let mut plugin_log = PluginLog::new("Csrf");
    assert_eq!(
        csrf.run_request(&mut mock_session, &mut plugin_log).await,
        PluginRunningResult::GoodNext
    );
    assert!(plugin_log.contains("OK"));
}

#[tokio::test]
async fn borrowed_cookie_preserves_field_order_and_credential_boundaries() {
    let csrf = Csrf::new(&create_csrf_config());
    let token = CsrfToken::generate(&csrf.key).encode().unwrap();
    let valid = format!("X-CSRF-Token={token}").into_bytes();
    let missing = PluginRunningResult::ErrResponse {
        status: 401,
        body: Some(Bytes::from_static(br#"{"error_msg":"no csrf cookie"}"#)),
    };
    let mismatch = PluginRunningResult::ErrResponse {
        status: 401,
        body: Some(Bytes::from_static(br#"{"error_msg":"csrf token mismatch"}"#)),
    };
    for (fields, expected, message) in [
        (
            vec![b"other=value".to_vec(), valid.clone()],
            PluginRunningResult::GoodNext,
            "OK",
        ),
        (
            vec![valid.clone(), b"opaque=\xff".to_vec()],
            PluginRunningResult::GoodNext,
            "OK",
        ),
        (
            vec![b"opaque=\xff".to_vec(), valid.clone()],
            missing.clone(),
            "Deny missing token",
        ),
        (vec![b"x-csrf-token=wrong-case".to_vec()], missing, "Deny missing token"),
        (
            vec![b"X-CSRF-Token=".to_vec(), valid.clone()],
            mismatch.clone(),
            "Deny bad token",
        ),
        (
            vec![b"X-CSRF-Token=first; X-CSRF-Token=second".to_vec(), valid.clone()],
            mismatch,
            "Deny bad token",
        ),
        (
            vec![format!("other=a=b; X-CSRF-Token={token}; tail=x").into_bytes()],
            PluginRunningResult::GoodNext,
            "OK",
        ),
    ] {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-csrf-token", http::HeaderValue::from_str(&token).unwrap());
        for field in fields {
            headers.append(http::header::COOKIE, http::HeaderValue::from_bytes(&field).unwrap());
        }
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("POST".to_string());
        session.expect_get_cookie().times(0);
        session.expect_request_header_map().return_const(headers);
        let mut log = PluginLog::new("Csrf");
        assert_eq!(csrf.run_request(&mut session, &mut log).await, expected);
        assert!(log.contains(message));
    }
}

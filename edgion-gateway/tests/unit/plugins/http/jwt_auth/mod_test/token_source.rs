//! Token extraction from header / query / cookie.

use super::common::*;
use serde_json::json;

#[tokio::test]
async fn test_token_from_query_param() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.query = "token".to_string();
            c.allow_token_in_query = true;
        }),
    );

    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let token = TOKEN.get_or_init(|| {
        generate_hs256_token(
            json!({
                "sub": "query-user", "exp": now_ts() + 3600
            }),
            TEST_SECRET,
        )
    });

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref()
        .withf(|name| name == "token")
        .times(1)
        .return_const(Some(token.as_str()));
    mock.expect_get_cookie().times(0);
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);
    assert!(log.contains("OK group="), "log should contain OK group=, got: {log:?}");
}

#[tokio::test]
async fn test_query_token_rejected_when_not_allowed_by_default() {
    // Default config has allow_token_in_query = false, so a token carried only in
    // the query parameter must be ignored and the request treated as unauthenticated
    // (RFC 6750 §2.3/§5.3). No anonymous fallback is configured, so this 401s.
    let auth = jwt_auth_with_credential(TEST_SECRET, JwtAlgorithm::HS256, None);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().times(0);
    mock.expect_get_cookie().times(0);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
    assert!(
        log.contains("No token"),
        "query token must be ignored by default, got: {log:?}"
    );
}

#[tokio::test]
async fn test_token_from_cookie() {
    let auth = jwt_auth_with_credential(
        TEST_SECRET,
        JwtAlgorithm::HS256,
        Some(|c| {
            c.cookie = "jwt_cookie".to_string();
        }),
    );

    let claims = json!({
        "sub": "cookie-user",
        "exp": now_ts() + 3600
    });
    let token = generate_hs256_token(claims, TEST_SECRET);

    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");

    mock.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock, &[("Cookie", &format!("jwt_cookie={token}"))]);
    mock.expect_get_query_param().times(0);
    mock.expect_get_query_param_ref().returning(|_| None);
    mock.expect_get_cookie().times(0);
    mock.expect_set_request_header().returning(|_, _| Ok(()));

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::GoodNext);
    assert!(log.contains("OK group="), "log should contain OK group=, got: {log:?}");
}

#[test]
fn borrowed_token_sources_preserve_priority_and_empty_rules() {
    use super::super::request_stage::TokenSource;
    for (header, query, allow_query, cookies, expected) in [
        (
            Some(b"  bEaReR   header-token  ".as_slice()),
            Some("query"),
            true,
            vec![b"jwt_cookie=cookie".as_slice()],
            Some(("header-token", "header")),
        ),
        (
            Some(b"Bearer ".as_slice()),
            Some("query"),
            true,
            vec![],
            Some(("Bearer", "header")),
        ),
        (
            Some(b"bad.token".as_slice()),
            Some("query"),
            true,
            vec![],
            Some(("bad.token", "header")),
        ),
        (
            Some(b"  ".as_slice()),
            Some("query"),
            true,
            vec![],
            Some(("query", "query")),
        ),
        (
            Some(b"\xff".as_slice()),
            Some("query"),
            true,
            vec![],
            Some(("query", "query")),
        ),
        (None, Some(" "), true, vec![], Some((" ", "query"))),
        (
            None,
            Some("query"),
            false,
            vec![b"jwt_cookie=cookie".as_slice()],
            Some(("cookie", "cookie")),
        ),
        (
            None,
            Some(""),
            true,
            vec![b"jwt_cookie=cookie=tail".as_slice()],
            Some(("cookie=tail", "cookie")),
        ),
        (
            None,
            None,
            true,
            vec![b"jwt_cookie=; jwt_cookie=later".as_slice()],
            None,
        ),
        (None, None, true, vec![b"JWT_COOKIE=wrong-case".as_slice()], None),
        (
            None,
            None,
            true,
            vec![b"opaque=\xff".as_slice(), b"jwt_cookie=later".as_slice()],
            None,
        ),
        (
            None,
            None,
            true,
            vec![b"jwt_cookie=first".as_slice(), b"opaque=\xff".as_slice()],
            Some(("first", "cookie")),
        ),
        (
            None,
            None,
            true,
            vec![b"jwt_cookie=  x".as_slice()],
            Some(("  x", "cookie")),
        ),
    ] {
        let mut auth = jwt_auth_default(None);
        auth.config.allow_token_in_query = allow_query;
        auth.config.cookie = "jwt_cookie".to_string();
        let mut headers = http::HeaderMap::new();
        if let Some(header) = header {
            headers.append(
                http::header::AUTHORIZATION,
                http::HeaderValue::from_bytes(header).unwrap(),
            );
            headers.append(
                http::header::AUTHORIZATION,
                http::HeaderValue::from_static("Bearer ignored-duplicate"),
            );
        }
        for cookie in cookies {
            headers.append(http::header::COOKIE, http::HeaderValue::from_bytes(cookie).unwrap());
        }
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        session.expect_get_query_param().times(0);
        session.expect_get_cookie().times(0);
        if allow_query {
            session
                .expect_get_query_param_ref()
                .withf(|name| name == "jwt")
                .return_const(query);
        } else {
            session.expect_get_query_param_ref().times(0);
        }
        let actual = auth.extract_token(&session).map(|(token, source)| {
            (
                token,
                match source {
                    TokenSource::Header => "header",
                    TokenSource::Query => "query",
                    TokenSource::Cookie => "cookie",
                },
            )
        });
        assert_eq!(actual, expected);
    }
}

#[test]
fn selected_tokens_borrow_their_existing_storage() {
    for source in ["header", "query", "cookie"] {
        let mut auth = jwt_auth_default(None);
        auth.config.allow_token_in_query = true;
        auth.config.cookie = "jwt_cookie".to_string();
        let mut headers = http::HeaderMap::new();
        let expected: &str = "selected-token";
        let pointer = match source {
            "header" => {
                headers.insert(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_static("Bearer selected-token"),
                );
                headers[http::header::AUTHORIZATION].to_str().unwrap()[7..].as_ptr()
            }
            "cookie" => {
                headers.insert(
                    http::header::COOKIE,
                    http::HeaderValue::from_static("jwt_cookie=selected-token"),
                );
                headers[http::header::COOKIE].to_str().unwrap()[11..].as_ptr()
            }
            _ => expected.as_ptr(),
        };
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        session.expect_get_query_param().times(0);
        session.expect_get_cookie().times(0);
        session
            .expect_get_query_param_ref()
            .return_const((source == "query").then_some(expected));
        let (token, _) = auth.extract_token(&session).unwrap();
        assert_eq!(token, expected);
        assert_eq!(token.as_ptr(), pointer);
    }
}

use std::collections::HashMap;

use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtAlgorithm, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};

use super::*;
use crate::plugins::http::common::auth_common::{CLAIM_HEADER_MUTATION_FAILED, CREDENTIAL_REMOVAL_FAILED};

fn assert_send_sync<T: Send + Sync>() {}

async fn projection_session() -> pingora_proxy::Session {
    crate::plugins::runtime::session_adapter::record_outcome_tests::test_session().await
}

fn signed_projection_token(sub: Option<&str>) -> String {
    let mut claims = serde_json::json!({"iss":"issuer-1", "key":"kid-1", "exp":4102444800u64});
    if let Some(sub) = sub {
        claims["sub"] = serde_json::json!(sub);
    }
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"prepared-secret-at-least-32-bytes-long"),
    )
    .unwrap()
}

#[tokio::test]
async fn configured_identity_headers_replace_all_input_or_remain_absent() {
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    for sub in [None, Some("trusted"), Some(""), Some("bad\r\nvalue")] {
        let mut config = config_with_resolved_credential();
        config.claims_to_headers = Some(HashMap::from([("sub".into(), "X-Consumer-Username".into())]));
        let auth = JwtAuth::new(&config, "default".into());
        let mut session = projection_session().await;
        session
            .req_header_mut()
            .insert_header("authorization", format!("Bearer {}", signed_projection_token(sub)))
            .unwrap();
        session
            .req_header_mut()
            .append_header("X-Consumer-Username", "administrator")
            .unwrap();
        session
            .req_header_mut()
            .append_header("x-consumer-username", "second-forgery")
            .unwrap();
        session.req_header_mut().insert_header("x-unrelated", "keep").unwrap();
        let mut ctx = EdgionHttpContext::default();
        assert_eq!(
            auth.run_request(
                &mut PingoraSessionAdapter::new(&mut session, &mut ctx),
                &mut PluginLog::new("JwtAuth")
            )
            .await,
            PluginRunningResult::GoodNext
        );
        let values: Vec<_> = session
            .req_header()
            .headers
            .get_all("x-consumer-username")
            .iter()
            .collect();
        if sub == Some("trusted") {
            assert_eq!(values.len(), 1);
            assert_eq!(values[0], "trusted");
            assert!(values[0].is_sensitive());
        } else {
            assert!(values.is_empty());
        }
        assert_eq!(session.req_header().headers["x-unrelated"], "keep");
    }
}

#[tokio::test]
async fn projection_consumes_token_before_clearing_same_named_destination() {
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    for hide in [false, true] {
        let mut config = config_with_resolved_credential();
        config.hide_credentials = hide;
        config.claims_to_headers = Some(HashMap::from([("sub".into(), "Authorization".into())]));
        let auth = JwtAuth::new(&config, "default".into());
        let mut session = projection_session().await;
        session
            .req_header_mut()
            .insert_header(
                "authorization",
                format!("Bearer {}", signed_projection_token(Some("trusted"))),
            )
            .unwrap();
        let mut ctx = EdgionHttpContext::default();
        assert_eq!(
            auth.run_request(
                &mut PingoraSessionAdapter::new(&mut session, &mut ctx),
                &mut PluginLog::new("JwtAuth")
            )
            .await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            session
                .req_header()
                .headers
                .get("authorization")
                .map(|v| v.to_str().unwrap()),
            if hide { None } else { Some("trusted") }
        );
    }
}

#[tokio::test]
async fn anonymous_clears_custom_claim_destinations_before_setting_its_identity() {
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    let mut config = JwtAuthConfig {
        anonymous: Some("guest".into()),
        ..Default::default()
    };
    config.claims_to_headers = Some(HashMap::from([
        ("sub".into(), "x-custom-identity".into()),
        ("name".into(), "X-Consumer-Username".into()),
    ]));
    let auth = JwtAuth::new(&config, "default".into());
    let mut session = projection_session().await;
    for name in ["x-custom-identity", "x-consumer-username"] {
        session.req_header_mut().append_header(name, "forged").unwrap();
        session.req_header_mut().append_header(name, "duplicate").unwrap();
    }
    let mut ctx = EdgionHttpContext::default();
    ctx.request_query.capture(None).unwrap();
    assert_eq!(
        auth.run_request(
            &mut PingoraSessionAdapter::new(&mut session, &mut ctx),
            &mut PluginLog::new("JwtAuth")
        )
        .await,
        PluginRunningResult::GoodNext
    );
    assert!(!session.req_header().headers.contains_key("x-custom-identity"));
    assert_eq!(session.req_header().headers["x-consumer-username"], "guest");
    assert_eq!(session.req_header().headers["x-anonymous-consumer"], "true");
}

#[tokio::test]
async fn claim_mutation_failures_never_continue_authenticated_or_anonymous_requests() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    for anonymous in [false, true] {
        for removal in [false, true] {
            let mut config = config_with_resolved_credential();
            config.anonymous = anonymous.then(|| "guest".into());
            config.claims_to_headers = Some(HashMap::from([("sub".into(), "x-sub".into())]));
            let auth = JwtAuth::new(&config, "default".into());
            let mut headers = http::HeaderMap::new();
            if !anonymous {
                headers.insert(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_str(&format!("Bearer {}", signed_projection_token(Some("trusted"))))
                        .unwrap(),
                );
            }
            let mut session = MockPluginSession::new();
            session.expect_request_header_map().return_const(headers);
            session.expect_get_query_param_ref().returning(|_| None);
            session.expect_remove_request_header().times(1).returning(move |_| {
                if removal {
                    Err(std::io::Error::other("private removal detail").into())
                } else {
                    Ok(())
                }
            });
            session
                .expect_set_request_header_value()
                .times(if removal { 0 } else { 1 })
                .returning(|_, _| Err(std::io::Error::other("private write detail").into()));
            // A token-present failure gets the bad-token challenge; anonymous gets the bare one.
            expect_header_deny(
                &mut session,
                if anonymous {
                    BARE_CHALLENGE
                } else {
                    INVALID_TOKEN_CHALLENGE
                },
            );
            let mut log = PluginLog::new("JwtAuth");
            assert_eq!(
                auth.run_request(&mut session, &mut log).await,
                PluginRunningResult::ErrTerminateRequest
            );
            assert!(log.contains(CLAIM_HEADER_MUTATION_FAILED));
            assert!(!log.contains("private"));
            assert!(!log.contains("OK group="), "got: {log:?}");
            assert!(!log.contains("Anon="), "got: {log:?}");
        }
    }
}

const BARE_CHALLENGE: &str = "Bearer realm=\"jwt\"";
const INVALID_TOKEN_CHALLENGE: &str =
    "Bearer realm=\"jwt\", error=\"invalid_token\", error_description=\"Unauthorized - Invalid or missing JWT\"";
const FAILURE_BODY: &[u8] = b"401 Unauthorized - Invalid or missing JWT";

/// Expect exactly one recorded `AuthHeaderMutationRejected` and the ordinary
/// 401 Bearer response with `challenge`, written in order.
fn expect_header_deny(
    session: &mut crate::plugins::runtime::traits::session::MockPluginSession,
    challenge: &'static str,
) {
    let mut sequence = mockall::Sequence::new();
    session
        .expect_record_error()
        .withf(|code| *code == edgion_resources::EdgionErrorCode::AuthHeaderMutationRejected)
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(());
    session
        .expect_write_response_header()
        .withf(move |response, end_of_stream| {
            !end_of_stream
                && response.status.as_u16() == 401
                && response
                    .headers
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok())
                    == Some(challenge)
        })
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_write_response_body()
        .withf(|body, end_of_stream| *end_of_stream && body.as_deref() == Some(FAILURE_BODY))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});
}

fn token_headers(token: &str) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}

#[tokio::test]
async fn hidden_header_credential_removal_failure_denies_with_invalid_token() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let mut config = config_with_resolved_credential();
    config.hide_credentials = true;
    let auth = JwtAuth::new(&config, "default".into());
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(token_headers(&signed_projection_token(Some("trusted"))));
    session.expect_get_query_param_ref().returning(|_| None);
    session
        .expect_remove_request_header()
        .withf(|name| name == "authorization")
        .times(1)
        .returning(|_| Err(std::io::Error::other("private removal detail").into()));
    expect_header_deny(&mut session, INVALID_TOKEN_CHALLENGE);
    let mut log = PluginLog::new("JwtAuth");
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains(CREDENTIAL_REMOVAL_FAILED), "got: {log:?}");
    assert!(!log.contains("private"));
    assert!(!log.contains("OK group="), "got: {log:?}");
}

#[tokio::test]
async fn hidden_query_credential_removal_failure_denies_with_invalid_token() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let mut config = config_with_resolved_credential();
    config.hide_credentials = true;
    config.allow_token_in_query = true;
    let auth = JwtAuth::new(&config, "default".into());
    // The mock returns a `'static` borrow.
    let token: &'static str = Box::leak(signed_projection_token(Some("trusted")).into_boxed_str());
    let mut session = MockPluginSession::new();
    session.expect_request_header_map().return_const(http::HeaderMap::new());
    session
        .expect_get_query_param_ref()
        .withf(|name| name == "jwt")
        .return_const(Some(token));
    session
        .expect_remove_request_query_param()
        .withf(|name| name == "jwt")
        .times(1)
        .returning(|_| Err(std::io::Error::other("private removal detail").into()));
    expect_header_deny(&mut session, INVALID_TOKEN_CHALLENGE);
    let mut log = PluginLog::new("JwtAuth");
    assert_eq!(
        auth.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains(CREDENTIAL_REMOVAL_FAILED), "got: {log:?}");
    assert!(!log.contains("OK group="), "got: {log:?}");
}

#[tokio::test(start_paused = true)]
async fn header_mutation_deny_applies_the_failure_delay() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use tokio::time::{advance, Duration, Instant};
    let mut config = config_with_resolved_credential();
    config.auth_failure_delay_ms = 100;
    config.claims_to_headers = Some(HashMap::from([("sub".into(), "x-sub".into())]));
    let auth = JwtAuth::new(&config, "default".into());
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(token_headers(&signed_projection_token(Some("trusted"))));
    session.expect_get_query_param_ref().returning(|_| None);
    session
        .expect_remove_request_header()
        .times(1)
        .returning(|_| Err(std::io::Error::other("injected").into()));
    expect_header_deny(&mut session, INVALID_TOKEN_CHALLENGE);

    let start = Instant::now();
    let task = tokio::spawn(async move {
        let mut log = PluginLog::new("JwtAuth");
        let result = auth.run_request(&mut session, &mut log).await;
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
    assert!(log.contains(CLAIM_HEADER_MUTATION_FAILED));
}

fn config_with_resolved_credential() -> JwtAuthConfig {
    let credential = ResolvedJwtCredential {
        key: Some("kid-1".to_string()),
        secret: Some("prepared-secret-at-least-32-bytes-long".to_string()),
        public_key: None,
    };
    let group = ResolvedJwtIssuerGroup {
        group: "group-1".to_string(),
        algorithm: JwtAlgorithm::HS256,
        audiences: None,
        credentials: HashMap::from([("kid-1".to_string(), credential)]),
    };
    JwtAuthConfig {
        resolved_groups_by_iss: Some(HashMap::from([("issuer-1".to_string(), group)])),
        ..Default::default()
    }
}

#[test]
fn construction_consumes_only_the_runtime_config_resolved_credentials() {
    let config = config_with_resolved_credential();
    let auth = JwtAuth::new(&config, "default".to_string());

    assert!(config.resolved_groups_by_iss.is_some());
    assert!(auth.config.resolved_groups_by_iss.is_none());
    let prepared = auth.store.lookup_by_iss("issuer-1").unwrap();
    assert_eq!(prepared.group, "group-1");
    assert!(prepared.lookup_decoding_key("kid-1").unwrap().is_ok());
}

#[test]
fn prepared_jwt_auth_is_send_and_sync() {
    assert_send_sync::<JwtAuth>();
}

#[tokio::test]
async fn authenticated_claim_sanitation_runs_for_missing_claim_and_fails_closed() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    for fail in [false, true] {
        let mut config = config_with_resolved_credential();
        config.claims_to_headers = Some(HashMap::from([("role".into(), "X-Role".into())]));
        let auth = JwtAuth::new(&config, "default".into());
        let token = encode(
            &Header::new(Algorithm::HS256),
            &serde_json::json!({"iss":"issuer-1", "key":"kid-1", "exp":4102444800u64}),
            &EncodingKey::from_secret(b"prepared-secret-at-least-32-bytes-long"),
        )
        .unwrap();
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        headers.insert("x-role", "admin".parse().unwrap());
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        session
            .expect_remove_request_header()
            .withf(|name| name == "x-role")
            .times(1)
            .returning(move |_| {
                if fail {
                    Err("injected removal failure".into())
                } else {
                    Ok(())
                }
            });
        session.expect_set_request_header_value().never();
        session.expect_set_ctx_var().returning(|_, _| Ok(()));
        if fail {
            expect_header_deny(&mut session, INVALID_TOKEN_CHALLENGE);
        }
        let mut log = PluginLog::new("JwtAuth");
        let result = auth.run_request(&mut session, &mut log).await;
        if fail {
            assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
            assert!(log.contains(CLAIM_HEADER_MUTATION_FAILED));
            assert!(!log.contains("OK group="), "got: {log:?}");
        } else {
            assert_eq!(result, PluginRunningResult::GoodNext);
            assert!(log.contains("OK group="), "got: {log:?}");
        }
    }
}

#[tokio::test]
async fn authenticated_cookie_cleanup_failure_returns_static_local_error() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::RequestFilter;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    for (error, status) in [
        (crate::request_cookie::CookieEditError::Unsanitizable, 400),
        (crate::request_cookie::CookieEditError::Internal, 500),
    ] {
        let mut config = config_with_resolved_credential();
        config.hide_credentials = true;
        config.cookie = "token".into();
        let auth = JwtAuth::new(&config, "default".into());
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("kid-1".into());
        let token = encode(
            &header,
            &serde_json::json!({"iss":"issuer-1", "key":"kid-1", "sub":"test-user", "exp":4102444800u64}),
            &EncodingKey::from_secret(b"prepared-secret-at-least-32-bytes-long"),
        )
        .unwrap();
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            http::HeaderValue::from_str(&format!("token={token}")).unwrap(),
        );
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        session.expect_get_query_param_ref().returning(|_| None);
        session.expect_set_ctx_var().returning(|_, _| Ok(()));
        session
            .expect_remove_request_cookies()
            .withf(|names| names == ["token"])
            .times(1)
            .returning(move |_| Err(error));
        session.expect_write_response_header().never();
        session.expect_write_response_body().never();
        let result = auth
            .run_request(&mut session, &mut crate::plugins::runtime::PluginLog::new("JwtAuth"))
            .await;
        assert_eq!(
            result,
            edgion_resources::filters::PluginRunningResult::ErrResponse { status, body: None }
        );
    }
}

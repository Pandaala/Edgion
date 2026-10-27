//! In-memory regressions for OIDC dependency-error response boundaries.

use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use serde_json::json;

pub(super) fn config() -> OpenidConnectConfig {
    OpenidConnectConfig {
        discovery: "https://idp.example.test/discovery".into(),
        client_id: "fixture-client".into(),
        resolved_oidc_client_secret: Some("private-client-secret".into()),
        resolved_session_secret: Some("private-session-secret-32-bytes-long".into()),
        bearer_only: true,
        verification_mode: VerificationMode::IntrospectionOnly,
        introspection_cache_ttl: 60,
        redirect_uri: Some("https://app.example.test/callback".into()),
        timeout: 2,
        ..Default::default()
    }
}

pub(super) fn discovery(origin: &str) -> DiscoveryDocument {
    DiscoveryDocument {
        issuer: origin.into(),
        jwks_uri: format!("{origin}/jwks"),
        authorization_endpoint: Some(format!("{origin}/authorize")),
        token_endpoint: Some(format!("{origin}/token")),
        introspection_endpoint: Some(format!("{origin}/introspect")),
        userinfo_endpoint: Some(format!("{origin}/userinfo")),
        end_session_endpoint: None,
        revocation_endpoint: Some(format!("{origin}/revoke")),
    }
}

pub(super) fn request(token: Option<&str>) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/resource".to_owned());
    session.expect_request_query().return_const(None);
    session.expect_downstream_is_tls().return_const(Some(true));
    let authorization = token.map(|token| format!("Bearer {token}"));
    match authorization.as_deref() {
        Some(value) => setup_header_mocks(&mut session, &[("authorization", value)]),
        None => setup_header_mocks(&mut session, &[]),
    }
    session
}

pub(super) fn assert_bad_gateway(result: PluginRunningResult) {
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 502,
            body: Some(Bytes::from_static(b"502 Bad Gateway")),
        }
    );
}

pub(super) fn assert_safe_log(log: &PluginLog) {
    let serialized = serde_json::to_string(log).unwrap();
    for forbidden in ["private-", "example.test", "127.0.0.1", "169.254", "503", "418"] {
        assert!(
            !serialized.contains(forbidden),
            "diagnostic exposed {forbidden}: {serialized}"
        );
    }
    assert!(serde_json::to_value(log).unwrap()["log"]
        .as_array()
        .is_some_and(|entries| !entries.is_empty()));
}

pub(super) fn expect_auth_response(session: &mut MockPluginSession, status: u16, text: &'static str) {
    session
        .expect_write_response_header()
        .times(1)
        .returning(move |header, end| {
            assert_eq!(header.status.as_u16(), status);
            assert!(!end);
            let challenge = header.headers["www-authenticate"].to_str().unwrap();
            assert!(challenge.starts_with("Bearer realm="));
            assert_eq!(challenge.contains("insufficient_scope"), status == 403);
            assert!(!challenge.contains("private-"));
            Ok(())
        });
    session
        .expect_write_response_body()
        .times(1)
        .returning(move |body, end| {
            assert!(end);
            assert_eq!(body.unwrap(), Bytes::from(format!("{status} {text}")));
            Ok(())
        });
    session.expect_shutdown().times(1).return_const(());
}

#[tokio::test]
async fn missing_and_invalid_authorization_endpoints_return_fixed_dependency_response() {
    for endpoint in [None, Some("http://[private-invalid-host".to_string())] {
        let mut cfg = config();
        cfg.bearer_only = false;
        cfg.unauth_action = UnauthAction::Auth;
        let plugin = OpenidConnect::new(&cfg, "default".into());
        let mut doc = discovery("https://idp.example.test");
        doc.authorization_endpoint = endpoint;
        plugin.jwks_provider.seed_discovery(doc).await;
        let mut session = request(None);
        session.expect_get_cookie().returning(|_| None);
        let mut log = PluginLog::new("OpenidConnect");
        // No response writer expectation: fixed dependency errors belong to runtime.
        assert_bad_gateway(plugin.run_request(&mut session, &mut log).await);
        assert_safe_log(&log);
    }
}

#[tokio::test]
async fn missing_and_blocked_introspection_endpoints_are_not_bearer_rejections() {
    for endpoint in [None, Some("http://169.254.169.254/private-metadata".into())] {
        let mut cfg = config();
        cfg.auth_failure_delay_ms = 60_000;
        let plugin = OpenidConnect::new(&cfg, "default".into());
        let mut doc = discovery("https://idp.example.test");
        doc.introspection_endpoint = endpoint;
        plugin.jwks_provider.seed_discovery(doc).await;
        let mut session = request(Some("private-access-token"));
        let mut log = PluginLog::new("OpenidConnect");
        let result = tokio::time::timeout(Duration::from_secs(1), plugin.run_request(&mut session, &mut log))
            .await
            .expect("dependency errors must not wait for the configured authentication delay");
        assert_bad_gateway(result);
        assert_safe_log(&log);
    }
}

#[tokio::test]
async fn authentication_delay_happens_before_any_response_write() {
    let mut cfg = config();
    cfg.auth_failure_delay_ms = 60_000;
    let plugin = OpenidConnect::new(&cfg, "default".into());
    let mut session = request(None);
    let mut log = PluginLog::new("OpenidConnect");
    // With no writer expectation, an early header write fails this test immediately.
    let future = plugin.run_request(&mut session, &mut log);
    futures::pin_mut!(future);
    assert!(futures::poll!(future).is_pending());
}

#[tokio::test]
async fn failed_authentication_response_write_is_terminal_and_never_retried() {
    for fail_header in [true, false] {
        let plugin = OpenidConnect::new(&config(), "default".into());
        let mut session = request(None);
        let mut log = PluginLog::new("OpenidConnect");
        session
            .expect_write_response_header()
            .times(1)
            .returning(move |header, _| {
                assert_eq!(header.status.as_u16(), 401);
                if fail_header {
                    Err(std::io::Error::other("private-writer-failure").into())
                } else {
                    Ok(())
                }
            });
        session
            .expect_write_response_body()
            .times(usize::from(!fail_header))
            .returning(|_, _| Err(std::io::Error::other("private-writer-failure").into()));
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert!(!serde_json::to_string(&log).unwrap().contains("private-writer"));
    }
}

#[tokio::test]
async fn callback_scope_rejection_remains_401_while_bearer_rejection_remains_403() {
    let mut cfg = config();
    cfg.required_scopes = Some(vec!["write".into()]);
    let plugin = OpenidConnect::new(&cfg, "default".into());
    let claims = json!({"active": true, "iss": "https://idp.example.test", "exp": OpenidConnect::now_unix_secs() + 3600, "scope": "read"});
    let error = plugin.validate_required_scopes(&claims).unwrap_err();
    assert_eq!(error.http_status(), 403);
    let mut callback = request(None);
    expect_auth_response(&mut callback, 401, "Insufficient scope");
    plugin.send_oidc_error(&mut callback, &error).await;

    plugin
        .jwks_provider
        .seed_discovery(discovery("https://idp.example.test"))
        .await;
    plugin.cache_introspection_claims("private-access-token", &claims).await;
    let mut bearer = request(Some("private-access-token"));
    expect_auth_response(&mut bearer, 403, "Insufficient scope");
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut bearer, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
}

#[tokio::test]
async fn missing_refresh_endpoint_preserves_all_session_unauth_actions() {
    for action in [UnauthAction::Pass, UnauthAction::Deny, UnauthAction::Auth] {
        let mut cfg = config();
        cfg.bearer_only = false;
        cfg.unauth_action = action;
        let plugin = OpenidConnect::new(&cfg, "default".into());
        let mut doc = discovery("https://idp.example.test");
        doc.token_endpoint = None;
        plugin.jwks_provider.seed_discovery(doc).await;
        let cookie = OidcSessionCookie {
            session_ref: "private-missing-cache-entry".into(),
            access_token: String::new(),
            created_at: OpenidConnect::now_unix_secs(),
            expires_at: None,
            id_token: None,
            refresh_token: Some("private-refresh-token".into()),
        };
        let encoded = OpenidConnect::encode_signed_cookie_payload(&cookie, plugin.session_cipher().unwrap()).unwrap();
        let mut session = request(None);
        session.expect_get_cookie().returning(move |_| Some(encoded.clone()));
        match action {
            UnauthAction::Pass => {
                session
                    .expect_remove_request_cookies()
                    .times(1)
                    .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Removed));
            }
            UnauthAction::Deny => expect_auth_response(&mut session, 401, "Unauthorized - Authentication required"),
            UnauthAction::Auth => {
                session
                    .expect_write_response_header()
                    .times(1)
                    .returning(|header, end| {
                        assert!(end);
                        assert_eq!(header.status.as_u16(), 302);
                        assert!(header.headers["location"]
                            .to_str()
                            .unwrap()
                            .starts_with("https://idp.example.test/authorize?"));
                        assert_eq!(header.headers.get_all("set-cookie").iter().count(), 1);
                        Ok(())
                    });
            }
        }
        let mut log = PluginLog::new("OpenidConnect");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            if matches!(action, UnauthAction::Pass) {
                PluginRunningResult::GoodNext
            } else {
                PluginRunningResult::ErrTerminateRequest
            }
        );
        assert_safe_log(&log);
        assert!(serde_json::to_string(&log).unwrap().contains("refresh"));
    }
}

#[test]
fn typed_jwt_errors_keep_authentication_status_and_static_text() {
    use jsonwebtoken::errors::{Error, ErrorKind};
    for (kind, message) in [
        (ErrorKind::ExpiredSignature, "Token expired"),
        (ErrorKind::InvalidToken, "Invalid token format"),
        (ErrorKind::InvalidIssuer, "Invalid token issuer"),
        (ErrorKind::InvalidAudience, "Invalid token audience"),
        (ErrorKind::ImmatureSignature, "Token not yet valid"),
        (ErrorKind::InvalidSignature, "Invalid token signature"),
        (ErrorKind::InvalidAlgorithm, "Invalid token algorithm"),
        (ErrorKind::InvalidKeyFormat, "Invalid token"),
    ] {
        let error = map_jwt_decode_error_typed(Error::from(kind));
        assert_eq!(error.http_status(), 401);
        assert_eq!(error.user_message(), message);
    }
}

#[test]
fn auto_retains_dependency_diagnostics_without_replacing_original_auth_rejection() {
    use super::super::error::{OidcDependencyFailure, OidcOperation};
    for failure in [
        OidcDependencyFailure::HttpStatus,
        OidcDependencyFailure::Timeout,
        OidcDependencyFailure::TlsSanMismatch,
        OidcDependencyFailure::ResponseTooLarge,
        OidcDependencyFailure::ResponseRead,
        OidcDependencyFailure::ResponseJson,
    ] {
        let dependency = OidcVerifyError::dependency(OidcOperation::Introspection, failure);
        let result = OidcVerifyError::InvalidFormat.with_introspection_failure(dependency);
        assert_eq!(result.http_status(), 401);
        assert_eq!(result.user_message(), "Invalid token format");
        assert_eq!(result.log_tag(), dependency.log_tag());
        assert_eq!(result.response_log_tag_or("fallback"), dependency.log_tag());
        assert_eq!(
            OidcVerifyError::Expired.with_introspection_failure(dependency),
            OidcVerifyError::Expired
        );
    }
    assert_eq!(OidcVerifyError::RefreshSingleflightTimeout.http_status(), 502);
    assert_eq!(
        OidcVerifyError::RefreshSingleflightTimeout.user_message(),
        "Bad Gateway"
    );
}

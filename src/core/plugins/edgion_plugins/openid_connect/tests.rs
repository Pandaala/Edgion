use super::{DiscoveryDocument, IntrospectionCacheEntry, JwksState, OidcSessionCookie, OpenidConnect};
use crate::core::plugins::edgion_plugins::common::http_client::get_http_client_with_ssl_verify;
use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
use crate::core::plugins::plugin_runtime::PluginLog;
use crate::core::plugins::plugin_runtime::RequestFilter;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{OpenidConnectConfig, UnauthAction, VerificationMode};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

fn base_config() -> OpenidConnectConfig {
    OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        ..Default::default()
    }
}

#[test]
fn test_can_refresh_min_interval() {
    let now = Instant::now();
    assert!(OpenidConnect::can_refresh(None, now, Duration::from_secs(10)));
    assert!(!OpenidConnect::can_refresh(
        Some(now),
        now + Duration::from_secs(5),
        Duration::from_secs(10)
    ));
    assert!(OpenidConnect::can_refresh(
        Some(now),
        now + Duration::from_secs(12),
        Duration::from_secs(10)
    ));
}

#[test]
fn test_jwks_state_default_is_empty() {
    let state = JwksState::default();
    assert!(state.set.is_none());
    assert!(state.expires_at.is_none());
    assert!(state.last_refresh_at.is_none());
}

#[tokio::test]
async fn test_bearer_only_without_token_returns_401() {
    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|_| None);
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_unauth_action_pass_without_token_allows_request() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Pass;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|_| None);

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn test_unauth_action_auth_without_token_redirects_to_idp() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    });

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/orders".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|name| match name {
        "authorization" => None,
        "x-forwarded-proto" => Some("https".to_string()),
        "x-forwarded-host" => None,
        "host" => Some("api.example.com".to_string()),
        _ => None,
    });
    session.expect_get_query().return_const(Some("x=1".to_string()));
    session.expect_write_response_header().returning(|resp, _| {
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(location.starts_with("https://idp.example.com/authorize?"));
        assert!(location.contains("response_type=code"));
        assert!(location.contains("client_id=client"));
        assert!(location.contains("scope=openid"));
        assert!(location.contains("state="));
        assert!(location.contains("redirect_uri=https%3A%2F%2Fapi.example.com%2F.edgion%2Foidc%2Fcallback"));

        let cookie = resp
            .headers
            .get("Set-Cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(cookie.starts_with("edgion_oidc_state="));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_unauth_action_auth_with_nonce_redirects_with_nonce_param() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.use_nonce = true;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    });

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/orders".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|name| match name {
        "authorization" => None,
        "x-forwarded-proto" => Some("https".to_string()),
        "x-forwarded-host" => None,
        "host" => Some("api.example.com".to_string()),
        _ => None,
    });
    session.expect_get_query().return_const(Some("x=1".to_string()));
    session.expect_write_response_header().returning(|resp, _| {
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(location.starts_with("https://idp.example.com/authorize?"));
        assert!(location.contains("state="));
        assert!(location.contains("nonce="));
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_authorization_params_appends_non_reserved_params() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.authorization_params = Some(HashMap::from([
        ("foo".to_string(), "bar".to_string()),
        ("prompt".to_string(), "login".to_string()),
    ]));

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    });

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/orders".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|name| match name {
        "authorization" => None,
        "x-forwarded-proto" => Some("https".to_string()),
        "x-forwarded-host" => None,
        "host" => Some("api.example.com".to_string()),
        _ => None,
    });
    session.expect_get_query().return_const(None);
    session.expect_write_response_header().returning(|resp, _| {
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(location.contains("response_type=code"));
        assert!(location.contains("foo=bar"));
        assert!(location.contains("prompt=login"));
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_callback_path_with_code_missing_state_returns_400() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session
        .expect_get_path()
        .return_const("/.edgion/oidc/callback".to_string());
    session
        .expect_get_query_param()
        .returning(|name| if name == "code" { Some("abc".to_string()) } else { None });
    session.expect_write_response_header().returning(|_, _| Ok(()));
    session.expect_write_response_body().returning(|_, _| Ok(()));
    session.expect_shutdown().returning(|| {});

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_logout_path_clears_cookies_and_redirects() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|name| match name {
        "x-forwarded-proto" => Some("https".to_string()),
        _ => None,
    });
    session.expect_write_response_header().returning(|resp, _| {
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert_eq!(location, "/");
        let cookie = resp
            .headers
            .get("Set-Cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(cookie.contains("Max-Age=0"));
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_logout_prefers_end_session_endpoint_when_cached() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/after-logout".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: Some("https://idp.example.com/logout".to_string()),
        revocation_endpoint: None,
    });

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|name| match name {
        "x-forwarded-proto" => Some("https".to_string()),
        "x-forwarded-host" => None,
        "host" => Some("api.example.com".to_string()),
        _ => None,
    });
    session.expect_write_response_header().returning(|resp, _| {
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(location.starts_with("https://idp.example.com/logout?"));
        assert!(location.contains("post_logout_redirect_uri=https%3A%2F%2Fapi.example.com%2Fafter-logout"));
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_logout_end_session_includes_id_token_hint_when_available() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/after-logout".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: Some("https://idp.example.com/logout".to_string()),
        revocation_endpoint: None,
    });

    let now = OpenidConnect::now_unix_secs();
    let session_cookie_payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: "access-token".to_string(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: Some("id-token-hint".to_string()),
        refresh_token: None,
    };
    let encoded_cookie = plugin
        .encode_signed_cookie_payload(&session_cookie_payload, "dummy-session-secret-32-bytes-long!!")
        .expect("encode session cookie");

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    session.expect_get_cookie().returning(move |name| {
        if name == "edgion_oidc_session" {
            Some(encoded_cookie.clone())
        } else {
            None
        }
    });
    session.expect_header_value().returning(|name| match name {
        "x-forwarded-proto" => Some("https".to_string()),
        "x-forwarded-host" => None,
        "host" => Some("api.example.com".to_string()),
        _ => None,
    });
    session.expect_write_response_header().returning(|resp, _| {
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(location.contains("post_logout_redirect_uri=https%3A%2F%2Fapi.example.com%2Fafter-logout"));
        assert!(location.contains("id_token_hint=id-token-hint"));
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn test_logout_with_revoke_enabled_and_no_session_cookie_still_redirects() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.revoke_tokens_on_logout = true;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    session.expect_get_cookie().returning(|_| None);
    session.expect_header_value().returning(|name| match name {
        "x-forwarded-proto" => Some("https".to_string()),
        _ => None,
    });
    session.expect_write_response_header().returning(|resp, _| {
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert_eq!(location, "/");
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[test]
fn test_signed_cookie_roundtrip() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let payload = super::AuthorizationStateCookie {
        state: "abc".to_string(),
        original_url: "/orders".to_string(),
        code_verifier: Some("verifier".to_string()),
        nonce: Some("nonce-value".to_string()),
        created_at: 123,
    };
    let encoded = plugin
        .encode_signed_cookie_payload(&payload, "dummy-session-secret-32-bytes-long!!")
        .expect("encode");
    let decoded: super::AuthorizationStateCookie = plugin
        .decode_signed_cookie_payload(&encoded, "dummy-session-secret-32-bytes-long!!")
        .expect("decode");
    assert_eq!(decoded.state, "abc");
    assert_eq!(decoded.original_url, "/orders");
}

#[tokio::test]
async fn test_maybe_set_session_cookie_header_strips_access_token_and_caches_it() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let now = OpenidConnect::now_unix_secs();
    let payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: "access-token".to_string(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };

    let captured_cookie = Arc::new(StdMutex::new(None::<String>));
    let captured_cookie_clone = captured_cookie.clone();

    let mut session = MockPluginSession::new();
    session.expect_set_response_header().returning(move |name, value| {
        if name.eq_ignore_ascii_case("Set-Cookie") {
            *captured_cookie_clone.lock().expect("lock") = Some(value.to_string());
        }
        Ok(())
    });

    plugin.maybe_set_session_cookie_header(&mut session, &payload).await;

    let set_cookie = captured_cookie
        .lock()
        .expect("lock")
        .clone()
        .expect("set-cookie header");
    let encoded = set_cookie
        .strip_prefix("edgion_oidc_session=")
        .and_then(|v| v.split(';').next())
        .unwrap_or("");
    let decoded: OidcSessionCookie = plugin
        .decode_signed_cookie_payload(encoded, "dummy-session-secret-32-bytes-long!!")
        .expect("decode persisted cookie");
    assert!(decoded.access_token.is_empty());
    assert_eq!(decoded.session_ref, "session-ref");
    assert_eq!(
        plugin.get_cached_access_token("session-ref").await,
        Some("access-token".to_string())
    );
}

#[tokio::test]
async fn test_resolve_access_token_from_session_prefers_cache_when_cookie_token_missing() {
    let cfg = base_config();
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let now = OpenidConnect::now_unix_secs();

    plugin
        .cache_access_token("session-ref", "cached-access-token", Some(now + 60))
        .await;

    let session_cookie = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: String::new(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };
    let resolved = plugin.resolve_access_token_from_session(&session_cookie).await;
    assert_eq!(resolved.as_deref(), Some("cached-access-token"));
}

#[tokio::test]
async fn test_run_request_uses_cached_session_access_token_when_cookie_token_missing() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Deny;
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.introspection_cache_ttl = 60;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: None,
        token_endpoint: None,
        introspection_endpoint: Some("https://idp.example.com/introspect".to_string()),
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    });

    let now = OpenidConnect::now_unix_secs();
    let token = "cached-access-token";
    plugin.cache_access_token("session-ref", token, Some(now + 60)).await;
    plugin
        .cache_introspection_claims(
            token,
            &json!({
                "active": true,
                "iss": "https://idp.example.com",
                "exp": now + 120
            }),
        )
        .await;

    let session_cookie_payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: String::new(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: None,
        refresh_token: None,
    };
    let encoded_cookie = plugin
        .encode_signed_cookie_payload(&session_cookie_payload, "dummy-session-secret-32-bytes-long!!")
        .expect("encode session cookie");

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(move |name| {
        if name == "edgion_oidc_session" {
            Some(encoded_cookie.clone())
        } else {
            None
        }
    });
    session.expect_header_value().returning(|_| None);

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn test_run_request_cache_miss_uses_refresh_singleflight_cached_result() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Deny;
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.introspection_cache_ttl = 60;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: None,
        token_endpoint: None,
        introspection_endpoint: Some("https://idp.example.com/introspect".to_string()),
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    });

    let now = OpenidConnect::now_unix_secs();
    let refreshed_token = "refreshed-access-token";
    plugin
        .cache_introspection_claims(
            refreshed_token,
            &json!({
                "active": true,
                "iss": "https://idp.example.com",
                "exp": now + 120
            }),
        )
        .await;

    let session_cookie_payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: String::new(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };
    let encoded_cookie = plugin
        .encode_signed_cookie_payload(&session_cookie_payload, "dummy-session-secret-32-bytes-long!!")
        .expect("encode session cookie");
    let lock_key = OpenidConnect::refresh_lock_key(&encoded_cookie);
    plugin
        .put_refresh_result(
            &lock_key,
            OidcSessionCookie {
                session_ref: "session-ref".to_string(),
                access_token: refreshed_token.to_string(),
                created_at: now,
                expires_at: Some(now + 120),
                id_token: None,
                refresh_token: Some("refresh-token".to_string()),
            },
        )
        .await;

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(move |name| {
        if name == "edgion_oidc_session" {
            Some(encoded_cookie.clone())
        } else {
            None
        }
    });
    session.expect_header_value().returning(|_| None);
    session
        .expect_set_response_header()
        .withf(|name, _| name.eq_ignore_ascii_case("Set-Cookie"))
        .returning(|_, _| Ok(()));

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[test]
fn test_looks_like_jwt() {
    let jwtish = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.e30.signature";
    let opaque = "opaque-token-value";
    assert!(OpenidConnect::looks_like_jwt(jwtish));
    assert!(!OpenidConnect::looks_like_jwt(opaque));
}

#[test]
fn test_should_auto_fallback_to_introspection() {
    assert!(OpenidConnect::should_auto_fallback_to_introspection(&(
        401,
        "Invalid token format".to_string()
    )));
    assert!(!OpenidConnect::should_auto_fallback_to_introspection(&(
        401,
        "Invalid token signature".to_string()
    )));
    assert!(!OpenidConnect::should_auto_fallback_to_introspection(&(
        502,
        "Invalid token format".to_string()
    )));
}

#[test]
fn test_is_refresh_wait_timeout() {
    assert!(OpenidConnect::is_refresh_wait_timeout(&(
        502,
        "Token refresh singleflight wait timeout".to_string()
    )));
    assert!(!OpenidConnect::is_refresh_wait_timeout(&(502, "other".to_string())));
}

#[test]
fn test_claims_audience_matches_string_and_array() {
    let allowed = vec!["api-a".to_string(), "api-b".to_string()];
    assert!(OpenidConnect::claims_audience_matches(Some(&json!("api-a")), &allowed));
    assert!(OpenidConnect::claims_audience_matches(
        Some(&json!(["other", "api-b"])),
        &allowed
    ));
    assert!(!OpenidConnect::claims_audience_matches(Some(&json!("other")), &allowed));
}

#[test]
fn test_validate_claims_from_value_checks_issuer_and_audience() {
    let mut cfg = base_config();
    cfg.issuers = Some(vec!["https://issuer.example.com".to_string()]);
    cfg.audiences = Some(vec!["api-a".to_string()]);
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let now = OpenidConnect::now_unix_secs();
    let claims = json!({
        "iss": "https://issuer.example.com",
        "aud": "api-a",
        "exp": now + 600,
        "nbf": now.saturating_sub(10),
    });
    assert!(plugin
        .validate_claims_from_value(&claims, Some("https://issuer.example.com"))
        .is_ok());
}

#[test]
fn test_validate_required_scopes_only_applies_to_bearer_only_mode() {
    let mut cfg = base_config();
    cfg.required_scopes = Some(vec!["api:read".to_string()]);
    cfg.bearer_only = false;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    assert!(plugin.validate_required_scopes(&json!({})).is_ok());

    let mut cfg_bearer = base_config();
    cfg_bearer.required_scopes = Some(vec!["api:read".to_string()]);
    cfg_bearer.bearer_only = true;
    let plugin_bearer = OpenidConnect::new(&cfg_bearer, "default".to_string());
    assert_eq!(
        plugin_bearer.validate_required_scopes(&json!({})),
        Err((403, "Insufficient scope".to_string()))
    );
}

#[test]
fn test_should_preemptive_refresh() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.renew_access_token_on_expiry = true;
    cfg.access_token_expires_leeway = 30;
    cfg.resolved_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let now = OpenidConnect::now_unix_secs();
    let near_exp = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: "a".to_string(),
        created_at: now,
        expires_at: Some(now + 10),
        id_token: None,
        refresh_token: Some("r".to_string()),
    };
    let far_exp = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: "a".to_string(),
        created_at: now,
        expires_at: Some(now + 1000),
        id_token: None,
        refresh_token: Some("r".to_string()),
    };
    assert!(plugin.should_preemptive_refresh(&near_exp));
    assert!(!plugin.should_preemptive_refresh(&far_exp));
}

#[test]
fn test_is_expired_within_leeway() {
    let mut cfg = base_config();
    cfg.access_token_expires_leeway = 10;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let now = OpenidConnect::now_unix_secs();
    assert!(plugin.is_expired_within_leeway(&json!({ "exp": now.saturating_sub(5) })));
    assert!(!plugin.is_expired_within_leeway(&json!({ "exp": now.saturating_sub(30) })));
}

#[test]
fn test_refresh_lock_key_is_stable() {
    let a = OpenidConnect::refresh_lock_key("cookie-value");
    let b = OpenidConnect::refresh_lock_key("cookie-value");
    let c = OpenidConnect::refresh_lock_key("cookie-value-2");
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn test_introspection_cache_key_is_stable() {
    let a = OpenidConnect::introspection_cache_key("token-value");
    let b = OpenidConnect::introspection_cache_key("token-value");
    let c = OpenidConnect::introspection_cache_key("token-value-2");
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[tokio::test]
async fn test_introspection_cache_put_and_get() {
    let mut cfg = base_config();
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let claims = json!({"active": true, "sub": "u1"});
    plugin.cache_introspection_claims("token-a", &claims).await;

    assert_eq!(
        plugin.get_cached_introspection_claims("token-a").await,
        Some(claims.clone())
    );
    assert_eq!(plugin.get_cached_introspection_claims("token-b").await, None);
}

#[tokio::test]
async fn test_introspection_cache_prunes_expired_entries_on_insert() {
    let mut cfg = base_config();
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let expired_key = OpenidConnect::introspection_cache_key("expired-token");
    {
        let mut cache = plugin.introspection_cache.write().await;
        cache.insert(
            expired_key.clone(),
            IntrospectionCacheEntry {
                claims: json!({"active": true, "sub": "old"}),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );
    }

    plugin
        .cache_introspection_claims("fresh-token", &json!({"active": true, "sub": "new"}))
        .await;

    let cache = plugin.introspection_cache.read().await;
    assert!(!cache.contains_key(&expired_key));
}

#[tokio::test]
async fn test_verify_token_auto_falls_back_to_introspection_cache_on_invalid_jwt_format() {
    let mut cfg = base_config();
    cfg.verification_mode = VerificationMode::Auto;
    cfg.use_jwks = true;
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    {
        let mut discovery = plugin.discovery_doc.write().await;
        *discovery = Some(DiscoveryDocument {
            issuer: "https://issuer.example.com".to_string(),
            jwks_uri: "https://issuer.example.com/jwks".to_string(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: Some("https://issuer.example.com/introspect".to_string()),
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        });
    }

    let token = "bad.header.payload";
    let now = OpenidConnect::now_unix_secs();
    let claims = json!({
        "active": true,
        "iss": "https://issuer.example.com",
        "exp": now + 120
    });
    plugin.cache_introspection_claims(token, &claims).await;

    let verified = plugin.verify_token(token).await.expect("verify via fallback");
    assert_eq!(verified, claims);
}

#[tokio::test]
async fn test_refresh_singleflight_wait_timeout() {
    let mut cfg = base_config();
    cfg.timeout = 1;
    cfg.bearer_only = false;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let lock_key = "refresh-lock";
    let lock = plugin.get_or_create_refresh_lock(lock_key).await;
    let _held_guard = lock.lock().await;

    let now = OpenidConnect::now_unix_secs();
    let mut cookie = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: "old-token".to_string(),
        created_at: now,
        expires_at: Some(now + 10),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };

    let result = plugin
        .try_refresh_session_token_singleflight(lock_key, &mut cookie)
        .await;
    assert_eq!(
        result,
        Err((502, "Token refresh singleflight wait timeout".to_string()))
    );
}

#[tokio::test]
async fn test_refresh_singleflight_wait_timeout_uses_recent_cached_result() {
    let mut cfg = base_config();
    cfg.timeout = 1;
    cfg.bearer_only = false;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let lock_key = "refresh-lock-with-cache";
    let now = OpenidConnect::now_unix_secs();
    plugin
        .put_refresh_result(
            lock_key,
            OidcSessionCookie {
                session_ref: "session-ref".to_string(),
                access_token: "cached-token".to_string(),
                created_at: now,
                expires_at: Some(now + 30),
                id_token: Some("id-token".to_string()),
                refresh_token: Some("refresh-token".to_string()),
            },
        )
        .await;

    let lock = plugin.get_or_create_refresh_lock(lock_key).await;
    let _held_guard = lock.lock().await;

    let mut cookie = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: "old-token".to_string(),
        created_at: now,
        expires_at: Some(now + 10),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };

    let result = plugin
        .try_refresh_session_token_singleflight(lock_key, &mut cookie)
        .await;
    assert_eq!(result, Ok("cached-token".to_string()));
    assert_eq!(cookie.access_token, "cached-token");
    assert_eq!(cookie.id_token.as_deref(), Some("id-token"));
}

#[tokio::test]
async fn test_try_get_recent_refresh_result_prunes_expired() {
    let cfg = base_config();
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let lock_key = "refresh-cache-key";

    let now = OpenidConnect::now_unix_secs();
    let payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: "cached-token".to_string(),
        created_at: now,
        expires_at: Some(now + 30),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };
    plugin.put_refresh_result(lock_key, payload).await;

    {
        let mut map = plugin.refresh_singleflight_results.lock().await;
        if let Some(entry) = map.get_mut(lock_key) {
            entry.at = Instant::now() - Duration::from_secs(6);
        }
    }

    assert!(plugin.try_get_recent_refresh_result(lock_key).await.is_none());
    let map = plugin.refresh_singleflight_results.lock().await;
    assert!(!map.contains_key(lock_key));
}

#[test]
fn test_session_cookie_size_limit_check() {
    let mut cfg = base_config();
    cfg.max_session_cookie_bytes = 32;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    assert!(plugin.ensure_session_cookie_size_limit("short-cookie").is_ok());
    assert!(plugin
        .ensure_session_cookie_size_limit("this-cookie-value-is-definitely-too-long")
        .is_err());
}

#[test]
fn test_cookie_attr_suffix_respects_cookie_config() {
    let mut cfg = base_config();
    cfg.session_cookie_same_site = "Strict".to_string();
    cfg.session_cookie_http_only = false;
    cfg.session_cookie_secure = false;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let session = MockPluginSession::new();

    let suffix = plugin.build_cookie_attr_suffix(&session, 60, "/");
    assert!(suffix.contains("SameSite=Strict"));
    assert!(!suffix.contains("HttpOnly"));
    assert!(!suffix.contains("Secure"));
}

#[test]
fn test_cookie_attr_suffix_sets_secure_when_enabled() {
    let mut cfg = base_config();
    cfg.session_cookie_secure = true;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let session = MockPluginSession::new();

    let suffix = plugin.build_cookie_attr_suffix(&session, 60, "/");
    assert!(suffix.contains("Secure"));
}

#[test]
fn test_should_persist_id_token_when_logout_endpoint_exists() {
    let mut cfg = base_config();
    cfg.set_id_token_header = false;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let discovery = DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: None,
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: Some("https://idp.example.com/logout".to_string()),
        revocation_endpoint: None,
    };
    assert!(plugin.should_persist_id_token(&discovery));
}

#[test]
fn test_apply_upstream_headers_sets_id_token_and_userinfo_headers() {
    let mut cfg = base_config();
    cfg.set_access_token_header = true;
    cfg.set_id_token_header = true;
    cfg.set_userinfo_header = true;
    cfg.access_token_in_authorization_header = true;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-Access-Token" && value == "access-token")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-ID-Token" && value == "id-token")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "Authorization" && value == "Bearer access-token")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-Userinfo" && value == "{\"sub\":\"userinfo-user\"}")
        .times(1)
        .returning(|_, _| Ok(()));

    plugin.apply_upstream_headers(
        &mut session,
        "access-token",
        Some("id-token"),
        Some("{\"sub\":\"userinfo-user\"}"),
        &json!({"sub": "user-1"}),
    );
}

#[test]
fn test_apply_upstream_headers_skips_oversized_id_token_header() {
    let mut cfg = base_config();
    cfg.set_id_token_header = true;
    cfg.max_header_value_bytes = 4;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    session.expect_set_request_header().never();
    plugin.apply_upstream_headers(
        &mut session,
        "access-token",
        Some("id-token-too-long"),
        None,
        &json!({"sub": "user-1"}),
    );
}

#[test]
fn test_apply_upstream_headers_rejects_unsafe_userinfo_header_value() {
    let mut cfg = base_config();
    cfg.set_userinfo_header = true;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    session.expect_set_request_header().never();
    plugin.apply_upstream_headers(
        &mut session,
        "access-token",
        None,
        Some("{\"sub\":\"user\n1\"}"),
        &json!({"sub": "user-1"}),
    );
}

#[tokio::test]
async fn test_fetch_userinfo_json_returns_none_without_userinfo_endpoint() {
    let mut cfg = base_config();
    cfg.set_userinfo_header = true;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    *plugin.discovery_doc.write().await = Some(DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: None,
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    });

    assert_eq!(plugin.fetch_userinfo_json("access-token").await, None);
}

#[test]
fn test_http_client_selection_respects_ssl_verify() {
    let mut secure_cfg = base_config();
    secure_cfg.ssl_verify = true;
    let secure_plugin = OpenidConnect::new(&secure_cfg, "default".to_string());

    let mut insecure_cfg = base_config();
    insecure_cfg.ssl_verify = false;
    let insecure_plugin = OpenidConnect::new(&insecure_cfg, "default".to_string());

    assert!(std::ptr::eq(
        secure_plugin.http_client(),
        get_http_client_with_ssl_verify(true)
    ));
    assert!(std::ptr::eq(
        insecure_plugin.http_client(),
        get_http_client_with_ssl_verify(false)
    ));
    assert!(!std::ptr::eq(
        secure_plugin.http_client(),
        insecure_plugin.http_client()
    ));
}

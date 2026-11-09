//! OIDC-01: absolute session deadlines, independent of renewable token expiry.

use super::super::jwks_provider::{ScriptedHttp, JWKS_PATH};
use super::tests::{base_config, install_request_cookies, plugin_log_entries};
use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use jsonwebtoken::Header;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

fn payload() -> OidcSessionCookie {
    OidcSessionCookie {
        access_token: "live-token".into(),
        created_at: 1000,
        session_expires_at: 1060,
        expires_at: Some(OpenidConnect::now_unix_secs() + 3600),
        id_token: Some("id-token-hint".into()),
        refresh_token: Some("refresh-token".into()),
    }
}

async fn plugin(now: u64, action: UnauthAction) -> (OpenidConnect, Arc<AtomicU64>) {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = action;
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".into());
    cfg.resolved_oidc_client_secret = Some("dummy-client-secret".into());
    cfg.redirect_uri = Some("https://app.example.com/callback".into());
    cfg.set_access_token_header = true;
    cfg.introspection_cache_ttl = 60;
    let clock = Arc::new(AtomicU64::new(now));
    let mut plugin = OpenidConnect::new(&cfg, "default".into());
    plugin.session_clock = Some(clock.clone());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".into(),
            jwks_uri: "https://idp.example.com/jwks".into(),
            authorization_endpoint: Some("https://idp.example.com/authorize".into()),
            token_endpoint: None,
            introspection_endpoint: Some("https://idp.example.com/introspect".into()),
            userinfo_endpoint: None,
            end_session_endpoint: Some("https://idp.example.com/logout".into()),
            revocation_endpoint: None,
        })
        .await;
    plugin
        .cache_introspection_claims(
            "live-token",
            &Arc::new(json!({
                "active": true, "iss": "https://idp.example.com", "exp": OpenidConnect::now_unix_secs() + 3600
            })),
        )
        .await;
    (plugin, clock)
}

fn request(plugin: &OpenidConnect, value: &impl serde::Serialize) -> MockPluginSession {
    let encoded = OpenidConnect::encode_signed_cookie_payload(value, plugin.session_cipher().unwrap()).unwrap();
    request_cookie(&encoded)
}

fn request_cookie(encoded: &str) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/resource".to_owned());
    session.expect_get_query_param().returning(|_| None);
    install_request_cookies(&mut session, &format!("app=keep; edgion_oidc_session={encoded}"));
    session
}

/// The enabled `X-Access-Token` output is removed once before any write.
fn expect_access_token_cleared(session: &mut MockPluginSession) {
    session
        .expect_remove_request_header()
        .withf(|name| name == "x-access-token")
        .times(1)
        .returning(|_| Ok(()));
}

fn expect_denied(session: &mut MockPluginSession) {
    super::dependency_error_tests::expect_auth_response(session, 401, "Unauthorized - Authentication required");
}

#[test]
fn absolute_deadline_boundaries_and_checked_creation() {
    let mut value = payload();
    assert_eq!(value.remaining_lifetime(1000).unwrap(), 60);
    assert_eq!(value.remaining_lifetime(1059).unwrap(), 1);
    for now in [999, 1060, 1061, u64::MAX] {
        assert!(value.remaining_lifetime(now).is_err());
    }
    for deadline in [0, 999, 1000] {
        value.session_expires_at = deadline;
        assert!(value.remaining_lifetime(1000).is_err());
    }
    value.created_at = 0;
    value.session_expires_at = 1060;
    assert!(value.remaining_lifetime(1).is_err());
    assert_eq!(OpenidConnect::session_deadline(1000, 60).unwrap(), 1060);
    assert_eq!(OpenidConnect::session_deadline(u64::MAX - 1, 1).unwrap(), u64::MAX);
    for (created, lifetime) in [(0, 60), (1000, 0), (1, u64::MAX), (u64::MAX, 1)] {
        assert!(OpenidConnect::session_deadline(created, lifetime).is_err());
    }
}

#[tokio::test]
async fn expired_cookie_cannot_use_live_token_or_refresh_result() {
    for now in [1059, 1060, 1061] {
        for token_in_cookie in [false, true] {
            let (plugin, _) = plugin(now, UnauthAction::Deny).await;
            let mut value = payload();
            if !token_in_cookie {
                value.access_token.clear();
            }
            let encoded =
                OpenidConnect::encode_signed_cookie_payload(&value, plugin.session_cipher().unwrap()).unwrap();
            let mut refreshed = value.clone();
            refreshed.access_token = "live-token".into();
            plugin
                .put_refresh_result(&OpenidConnect::refresh_lock_key(&encoded), refreshed)
                .await;
            let mut session = request_cookie(&encoded);
            if now < 1060 {
                expect_access_token_cleared(&mut session);
                session
                    .expect_set_request_header_value()
                    .times(1)
                    .returning(|name, token| {
                        assert_eq!(name, "X-Access-Token");
                        assert_eq!(token, "live-token");
                        Ok(())
                    });
                session
                    .expect_append_response_header()
                    // Refreshed single cookie plus the chunk-count expiration.
                    .times(2 * usize::from(!token_in_cookie))
                    .returning(|_, cookie| {
                        assert!(
                            (cookie.starts_with("edgion_oidc_session=") && cookie.contains("Max-Age=1;"))
                                || cookie.starts_with("edgion_oidc_session_chunks=; ")
                        );
                        Ok(())
                    });
            } else {
                expect_denied(&mut session);
                session.expect_set_request_header_value().times(0);
                session.expect_append_response_header().times(0);
            }
            let mut log = PluginLog::new("OpenidConnect");
            let result = plugin.run_request(&mut session, &mut log).await;
            if now < 1060 {
                assert_eq!(result, PluginRunningResult::GoodNext);
            } else {
                assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
                assert_eq!(plugin_log_entries(&log), vec!["no token; deny"]);
            }
        }
    }
}

#[tokio::test]
async fn malformed_and_future_session_times_fail_closed() {
    let (plugin, _) = plugin(1000, UnauthAction::Deny).await;
    for (field, replacement) in [
        ("session_expires_at", None),
        ("session_expires_at", Some(json!("1060"))),
        ("session_expires_at", Some(json!(-1))),
        ("session_expires_at", Some(json!(1000))),
        ("created_at", Some(json!(1001))),
        ("created_at", Some(json!(u64::MAX))),
    ] {
        let mut value = serde_json::to_value(payload()).unwrap();
        if let Some(replacement) = replacement {
            value[field] = replacement;
        } else {
            value.as_object_mut().unwrap().remove(field);
        }
        let mut session = request(&plugin, &value);
        expect_denied(&mut session);
        let result = plugin
            .run_request(&mut session, &mut PluginLog::new("OpenidConnect"))
            .await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }
}

#[tokio::test]
async fn refreshed_cookie_preserves_deadline_and_uses_remaining_max_age() {
    let (plugin, clock) = plugin(1030, UnauthAction::Deny).await;
    let value = payload();
    let mut session = MockPluginSession::new();
    // Only the first write publishes: the session cookie and the chunk-count expiration.
    session
        .expect_append_response_header()
        .times(2)
        .returning(|name, header| {
            assert_eq!(name, "Set-Cookie");
            assert!(header.contains("Max-Age=30;") || header.starts_with("edgion_oidc_session_chunks=; "));
            Ok(())
        });
    let mut log = PluginLog::new("OpenidConnect");
    plugin
        .maybe_set_session_cookie_header(&mut session, &value, 0, &mut log)
        .await;
    assert_eq!(value.session_expires_at, 1060);
    clock.store(1060, Ordering::SeqCst);
    plugin
        .maybe_set_session_cookie_header(&mut session, &value, 0, &mut log)
        .await;
    assert!(plugin_log_entries(&log).is_empty());
}

#[tokio::test]
async fn session_expiring_during_refresh_wait_cannot_authenticate() {
    let (plugin, clock) = plugin(1059, UnauthAction::Deny).await;
    let mut value = payload();
    value.access_token.clear();
    let encoded = OpenidConnect::encode_signed_cookie_payload(&value, plugin.session_cipher().unwrap()).unwrap();
    let key = OpenidConnect::refresh_lock_key(&encoded);
    let flight = plugin.refresh_singleflight.get_or_create(&key);
    let guard = flight.lock().lock().await;
    let mut refreshed = value.clone();
    refreshed.access_token = "live-token".into();
    plugin.put_refresh_result(&key, refreshed).await;
    let mut session = request_cookie(&encoded);
    expect_denied(&mut session);
    session.expect_append_response_header().times(0);
    let mut log = PluginLog::new("OpenidConnect");
    let future = plugin.run_request(&mut session, &mut log);
    tokio::pin!(future);
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(future.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    clock.store(1060, Ordering::SeqCst);
    drop(guard);
    assert_eq!(future.await, PluginRunningResult::ErrTerminateRequest);
}

#[tokio::test]
async fn config_generation_changes_do_not_extend_or_shorten_signed_deadline() {
    let (mut plugin, clock) = plugin(1059, UnauthAction::Deny).await;
    for lifetime in [1, 86400] {
        let mut cfg = plugin.config.clone();
        // Prepared secrets have been consumed by the old runtime constructor.
        cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".into());
        cfg.resolved_oidc_client_secret = Some("dummy-client-secret".into());
        cfg.session_lifetime = lifetime;
        let mut next = OpenidConnect::new(&cfg, "default".into());
        next.session_clock = Some(clock.clone());
        let mut session = request(&plugin, &payload());
        let decoded = next.extract_session_cookie_from_request(&session).unwrap().unwrap().0;
        assert_eq!(decoded.remaining_lifetime(next.session_now()).unwrap(), 1);
        clock.store(1060, Ordering::SeqCst);
        expect_denied(&mut session);
        assert_eq!(
            next.run_request(&mut session, &mut PluginLog::new("OpenidConnect"))
                .await,
            PluginRunningResult::ErrTerminateRequest
        );
        clock.store(1059, Ordering::SeqCst);
        plugin = next;
    }
}

#[tokio::test]
async fn expired_cookie_preserves_pass_and_redirect_contracts() {
    for action in [UnauthAction::Pass, UnauthAction::Auth] {
        let (plugin, _) = plugin(1060, action).await;
        let mut session = request(&plugin, &payload());
        if action == UnauthAction::Pass {
            expect_access_token_cleared(&mut session);
        }
        if action == UnauthAction::Auth {
            session.expect_request_query().return_const(None);
            session
                .expect_write_response_header()
                .times(1)
                .returning(|header, end| {
                    assert!(end);
                    assert_eq!(header.status.as_u16(), 302);
                    assert!(header.headers["location"]
                        .to_str()
                        .unwrap()
                        .starts_with("https://idp.example.com/authorize?"));
                    Ok(())
                });
        }
        let result = plugin
            .run_request(&mut session, &mut PluginLog::new("OpenidConnect"))
            .await;
        assert_eq!(
            result,
            if action == UnauthAction::Pass {
                PluginRunningResult::GoodNext
            } else {
                PluginRunningResult::ErrTerminateRequest
            }
        );
    }
}

#[tokio::test]
async fn verification_crossing_deadline_does_not_publish_identity() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Deny;
    cfg.allowed_signing_algs = Some(vec!["HS256".into()]);
    cfg.token_signing_alg = Some("HS256".into());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".into());
    let mut plugin = OpenidConnect::new(&cfg, "default".into());
    let clock = Arc::new(AtomicU64::new(1059));
    plugin.session_clock = Some(clock.clone());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".into(),
            jwks_uri: "https://idp.example.com/jwks".into(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;
    plugin
        .jwks_provider
        .seed_stale_jwks(
            serde_json::from_value(json!({
                "keys": [{"kty": "oct", "k": "c2VjcmV0", "alg": "HS256"}]
            }))
            .unwrap(),
        )
        .await;
    let script = ScriptedHttp::new();
    script.enqueue_transport(JWKS_PATH);
    plugin.jwks_provider.install_scripted_http(script);
    let mut value = payload();
    value.access_token = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &json!({
            "iss": "https://idp.example.com", "exp": OpenidConnect::now_unix_secs() + 3600
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"secret"),
    )
    .unwrap();
    let mut session = request(&plugin, &value);
    session
        .expect_record_outcome()
        .times(1)
        .returning(move |outcome, reason| {
            assert_eq!(outcome, DyeOutcome::FailOpen);
            assert_eq!(reason, "jwks-stale-cache");
            clock.store(1060, Ordering::SeqCst);
        });
    expect_denied(&mut session);
    session.expect_set_request_header_value().times(0);
    session.expect_set_ctx_var().times(0);
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(plugin_log_entries(&log), vec!["no token; deny"]);
}

#[tokio::test]
async fn expired_session_logout_keeps_id_hint_and_expires_all_session_cookies() {
    let (plugin, _) = plugin(1060, UnauthAction::Deny).await;
    let encoded = OpenidConnect::encode_signed_cookie_payload(&payload(), plugin.session_cipher().unwrap()).unwrap();
    let mut session = MockPluginSession::new();
    session
        .expect_get_path()
        .return_const(plugin.config.logout_path.clone());
    install_request_cookies(&mut session, &format!("edgion_oidc_session={encoded}"));
    session
        .expect_write_response_header()
        .times(1)
        .returning(|header, end| {
            assert!(end);
            assert_eq!(header.status.as_u16(), 302);
            assert!(header.headers["location"]
                .to_str()
                .unwrap()
                .contains("id_token_hint=id-token-hint"));
            let cookies: Vec<String> = header
                .headers
                .get_all("set-cookie")
                .iter()
                .map(|cookie| cookie.to_str().unwrap().to_string())
                .collect();
            assert_eq!(
                cookies,
                super::tests::expected_logout_set_cookies_with_state_path("/callback")
            );
            Ok(())
        });
    assert_eq!(
        plugin
            .run_request(&mut session, &mut PluginLog::new("OpenidConnect"))
            .await,
        PluginRunningResult::ErrTerminateRequest
    );
}

#[tokio::test]
async fn valid_bearer_ignores_expired_session_deadline() {
    let (mut plugin, _) = plugin(1060, UnauthAction::Deny).await;
    plugin.config.bearer_only = true;
    let encoded = OpenidConnect::encode_signed_cookie_payload(&payload(), plugin.session_cipher().unwrap()).unwrap();
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/resource".to_owned());
    setup_header_mocks(
        &mut session,
        &[
            ("authorization", "Bearer live-token"),
            ("cookie", &format!("edgion_oidc_session={encoded}")),
        ],
    );
    session.expect_get_cookie().times(0);
    expect_access_token_cleared(&mut session);
    session
        .expect_set_request_header_value()
        .times(1)
        .returning(|name, token| {
            assert_eq!(name, "X-Access-Token");
            assert_eq!(token, "live-token");
            Ok(())
        });
    session.expect_remove_request_cookies().times(1).returning(|names| {
        assert_eq!(names, super::tests::gateway_owned_cookie_names().as_slice());
        Ok(crate::request_cookie::CookieEditOutcome::Removed)
    });
    assert_eq!(
        plugin
            .run_request(&mut session, &mut PluginLog::new("OpenidConnect"))
            .await,
        PluginRunningResult::GoodNext
    );
}

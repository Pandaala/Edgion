//! Explicit local-IdP tests for dependency error producers and public responses.
//!
//! Standalone topology exception: an ephemeral IdP exercises the real OIDC
//! outbound methods; no second Gateway/Controller or shared harness is needed.
//! Run: cargo test -p edgion-gateway --lib dependency_error_integration -- --ignored

use super::dependency_error_tests::{
    assert_bad_gateway, assert_safe_log, config, discovery, expect_auth_response, request,
};
use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use serde_json::json;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Copy, Debug)]
enum Endpoint {
    Discovery,
    Jwks,
    Exchange,
    Refresh,
    Introspection,
}

async fn dependency_failure(endpoint: Endpoint, response: ResponseTemplate) {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(response)
        .expect(1)
        .mount(&server)
        .await;
    let mut cfg = config();
    cfg.discovery = format!("{}/private-discovery", server.uri());
    cfg.discovery_max_response_bytes = 256;
    cfg.jwks_max_response_bytes = 256;
    let plugin = OpenidConnect::new(&cfg, "default".into());
    let doc = discovery(&server.uri());
    if !matches!(endpoint, Endpoint::Discovery) {
        plugin.jwks_provider.seed_discovery(doc.clone()).await;
    }
    let mut session = MockPluginSession::new();
    session.expect_record_outcome().return_const(());
    let error = match endpoint {
        Endpoint::Discovery => OidcVerifyError::Dependency(plugin.get_or_fetch_discovery().await.unwrap_err()),
        Endpoint::Jwks => OidcVerifyError::Dependency(plugin.get_or_fetch_jwks(&mut session, false).await.unwrap_err()),
        Endpoint::Exchange => plugin
            .exchange_code_for_token(&doc, "private-code", "https://app.example.test/callback", None)
            .await
            .unwrap_err(),
        Endpoint::Refresh => plugin
            .refresh_access_token(&doc, "private-refresh-token")
            .await
            .unwrap_err(),
        Endpoint::Introspection => plugin
            .verify_token_via_introspection(&mut session, "private-access-token")
            .await
            .unwrap_err(),
    };
    assert_eq!(error.http_status(), 502, "{endpoint:?}");
    assert_eq!(error.user_message(), "Bad Gateway", "{endpoint:?}");
    let debug = format!("{error:?}");
    for forbidden in ["private-", "127.0.0.1", "503", "418"] {
        assert!(
            !debug.contains(forbidden),
            "{endpoint:?} retained provider detail: {debug}"
        );
    }
    let mut log = PluginLog::new("OpenidConnect");
    log.push_err(error.log_tag());
    assert_safe_log(&log);
    // A fixed response must not invoke the dynamic response writer or challenge.
    assert_bad_gateway(plugin.send_oidc_error(&mut session, &error).await);
    server.verify().await;
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn all_blocking_http_operations_redact_provider_status_and_body() {
    for endpoint in [
        Endpoint::Discovery,
        Endpoint::Jwks,
        Endpoint::Exchange,
        Endpoint::Refresh,
        Endpoint::Introspection,
    ] {
        for status in [418, 503] {
            dependency_failure(
                endpoint,
                ResponseTemplate::new(status).set_body_string("private-provider-error at private-internal-host"),
            )
            .await;
        }
    }
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn all_blocking_http_operations_redact_json_parser_errors() {
    for endpoint in [
        Endpoint::Discovery,
        Endpoint::Jwks,
        Endpoint::Exchange,
        Endpoint::Refresh,
        Endpoint::Introspection,
    ] {
        dependency_failure(
            endpoint,
            ResponseTemplate::new(200).set_body_string("{private-provider-json-error"),
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn discovery_and_jwks_keep_their_configured_response_caps() {
    for endpoint in [Endpoint::Discovery, Endpoint::Jwks] {
        dependency_failure(endpoint, ResponseTemplate::new(200).set_body_string(" ".repeat(257))).await;
    }
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn auto_introspection_status_failure_keeps_original_401_and_safe_diagnostic() {
    let server = MockServer::start().await;
    Mock::given(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "keys": [{"kty":"oct", "kid":"fixture", "k":"Zml4dHVyZS1rZXk", "alg":"HS256"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/introspect"))
        .respond_with(ResponseTemplate::new(503).set_body_string("private-provider-error"))
        .expect(1)
        .mount(&server)
        .await;
    let mut cfg = config();
    cfg.verification_mode = VerificationMode::Auto;
    cfg.allowed_signing_algs = Some(vec!["HS256".into()]);
    cfg.use_jwks = true;
    let plugin = OpenidConnect::new(&cfg, "default".into());
    plugin.jwks_provider.seed_discovery(discovery(&server.uri())).await;
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("fixture".into());
    // A valid signature with non-object claims takes the real InvalidFormat fallback.
    let token = jsonwebtoken::encode(
        &header,
        &json!("private-invalid-claims"),
        &jsonwebtoken::EncodingKey::from_secret(b"fixture-key"),
    )
    .unwrap();
    let mut session = request(Some(&token));
    expect_auth_response(&mut session, 401, "Invalid token format");
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_safe_log(&log);
    assert!(serde_json::to_string(&log).unwrap().contains("introspect"));
    server.verify().await;
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn introspection_status_failure_flows_through_request_filter_as_fixed_502() {
    let server = MockServer::start().await;
    Mock::given(path("/introspect"))
        .respond_with(ResponseTemplate::new(503).set_body_string("private-provider-error"))
        .expect(1)
        .mount(&server)
        .await;
    let mut cfg = config();
    cfg.auth_failure_delay_ms = 60_000;
    let plugin = OpenidConnect::new(&cfg, "default".into());
    plugin.jwks_provider.seed_discovery(discovery(&server.uri())).await;
    let mut session = request(Some("private-access-token"));
    let mut log = PluginLog::new("OpenidConnect");
    let result = tokio::time::timeout(Duration::from_secs(3), plugin.run_request(&mut session, &mut log))
        .await
        .unwrap();
    assert_bad_gateway(result);
    assert_safe_log(&log);
    server.verify().await;
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn userinfo_failure_keeps_authenticated_request_and_claim_header_fallback() {
    for response in [
        ResponseTemplate::new(503).set_body_string("private-provider-error"),
        ResponseTemplate::new(200).set_body_string("{private-parser-error"),
        ResponseTemplate::new(200).set_body_string(" ".repeat(257)),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/userinfo"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let mut cfg = config();
        cfg.set_userinfo_header = true;
        cfg.userinfo_max_response_bytes = 256;
        let plugin = OpenidConnect::new(&cfg, "default".into());
        plugin.jwks_provider.seed_discovery(discovery(&server.uri())).await;
        let claims = Arc::new(
            json!({"active":true, "iss":server.uri(), "sub":"fixture-user", "exp":OpenidConnect::now_unix_secs() + 3600}),
        );
        plugin.cache_introspection_claims("private-access-token", &claims).await;
        let mut session = request(Some("private-access-token"));
        session
            .expect_set_request_header_value()
            .times(1)
            .returning(move |name, value| {
                assert_eq!(name, "X-Userinfo");
                assert!(value.is_sensitive());
                assert_eq!(serde_json::from_slice::<Value>(value.as_bytes()).unwrap(), *claims);
                Ok(())
            });
        let mut log = PluginLog::new("OpenidConnect");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_safe_log(&log);
        assert!(serde_json::to_string(&log).unwrap().contains("userinfo"));
        server.verify().await;
    }
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn revocation_failure_still_clears_logout_cookies_and_access_cache() {
    let server = MockServer::start().await;
    Mock::given(path("/revoke"))
        .respond_with(ResponseTemplate::new(503).set_body_string("private-provider-error"))
        .expect(2)
        .mount(&server)
        .await;
    let mut cfg = config();
    cfg.bearer_only = false;
    cfg.revoke_tokens_on_logout = true;
    cfg.post_logout_redirect_uri = Some("https://app.example.test/logged-out".into());
    let plugin = OpenidConnect::new(&cfg, "default".into());
    plugin.jwks_provider.seed_discovery(discovery(&server.uri())).await;
    let cookie = OidcSessionCookie {
        session_ref: "private-session-ref".into(),
        access_token: String::new(),
        created_at: OpenidConnect::now_unix_secs(),
        session_expires_at: OpenidConnect::now_unix_secs() + 3600,
        expires_at: None,
        id_token: None,
        refresh_token: Some("private-refresh-token".into()),
    };
    plugin
        .cache_access_token(&cookie.session_ref, "private-access-token", None)
        .await;
    let encoded = OpenidConnect::encode_signed_cookie_payload(&cookie, plugin.session_cipher().unwrap()).unwrap();
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const(cfg.logout_path.clone());
    session.expect_get_cookie().returning(move |_| Some(encoded.clone()));
    session.expect_downstream_is_tls().return_const(Some(true));
    session
        .expect_write_response_header()
        .times(1)
        .returning(|header, end| {
            assert_eq!(header.status.as_u16(), 302);
            assert!(end);
            assert_eq!(header.headers["location"], "https://app.example.test/logged-out");
            let cookies: Vec<_> = header.headers.get_all("set-cookie").iter().collect();
            assert_eq!(cookies.len(), 2);
            for cookie in cookies {
                assert!(cookie.to_str().unwrap().contains("Max-Age=0"));
                assert!(!cookie.to_str().unwrap().contains("private-"));
            }
            Ok(())
        });
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(plugin.get_cached_access_token("private-session-ref").await.is_none());
    assert_safe_log(&log);
    assert!(serde_json::to_string(&log).unwrap().contains("revocation"));
    server.verify().await;
}

#[tokio::test]
#[ignore = "network integration test: explicit local IdP fixture"]
async fn failed_preemptive_refresh_does_not_invalidate_existing_valid_token() {
    let server = MockServer::start().await;
    Mock::given(path("/token"))
        .respond_with(ResponseTemplate::new(503).set_body_string("private-provider-error"))
        .expect(1)
        .mount(&server)
        .await;
    let mut cfg = config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Deny;
    cfg.access_token_expires_leeway = 120;
    let plugin = OpenidConnect::new(&cfg, "default".into());
    plugin.jwks_provider.seed_discovery(discovery(&server.uri())).await;
    let expiry = OpenidConnect::now_unix_secs() + 60;
    plugin
        .cache_introspection_claims(
            "private-access-token",
            &Arc::new(json!({"active":true, "iss":server.uri(), "exp":expiry})),
        )
        .await;
    let cookie: OidcSessionCookie = OidcSessionCookie {
        session_ref: "private-session-ref".into(),
        access_token: "private-access-token".into(),
        created_at: OpenidConnect::now_unix_secs(),
        session_expires_at: OpenidConnect::now_unix_secs() + 3600,
        expires_at: Some(expiry),
        id_token: None,
        refresh_token: Some("private-refresh-token".into()),
    };
    let encoded = OpenidConnect::encode_signed_cookie_payload(&cookie, plugin.session_cipher().unwrap()).unwrap();
    let mut session = request(None);
    session.expect_get_cookie().returning(move |_| Some(encoded.clone()));
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_safe_log(&log);
    let log = serde_json::to_string(&log).unwrap();
    assert!(log.contains("refresh"));
    assert!(log.contains("session token verified"));
    server.verify().await;
}

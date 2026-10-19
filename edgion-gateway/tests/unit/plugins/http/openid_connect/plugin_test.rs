use super::super::jwt_common::{select_jwk, JwkSelectError};
use super::{DiscoveryDocument, IntrospectionCacheEntry, OidcSessionCookie, OpenidConnect};
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::plugins::runtime::DyeOutcome;
use crate::plugins::runtime::PluginLog;
use crate::plugins::runtime::RequestFilter;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::path_normalization::{normalize_path, PathNormalizationConfig};
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::edgion_plugins::{OpenidConnectConfig, UnauthAction, VerificationMode};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, Header};
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn actual_oidc(config: &OpenidConnectConfig, namespace: String) -> Result<OpenidConnect, String> {
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(json!({
        "metadata":{"name":"oidc-test","namespace":namespace},
        "spec":{"requestPlugins":[{"type":"OpenidConnect","config":config}]}
    }))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    OpenidConnect::try_new(
        &entry,
        &edgion_common::link_sys_core::outbound_tls::outbound_global_default(),
    )
}

impl OpenidConnect {
    pub(super) fn new(config: &OpenidConnectConfig, namespace: String) -> Self {
        actual_oidc(config, namespace).expect("test OIDC configuration must build")
    }
}

#[test]
fn credential_dto_debug_omits_all_fields_and_preserves_transport() {
    let state: super::AuthorizationStateCookie = serde_json::from_value(json!({
        "state": "private-state", "original_url": "/private-url", "code_verifier": "private-verifier",
        "nonce": "private-nonce", "created_at": 1
    }))
    .unwrap();
    let session: OidcSessionCookie = serde_json::from_value(json!({
        "session_ref": "private-session", "access_token": "private-access", "created_at": 1,
        "id_token": "private-id", "refresh_token": "private-refresh"
    }))
    .unwrap();
    let response: super::TokenEndpointResponse = serde_json::from_value(json!({
        "access_token": "private-access", "id_token": "private-id", "refresh_token": "private-refresh"
    }))
    .unwrap();
    let introspection = IntrospectionCacheEntry {
        claims: json!({"sub": "private-identity"}),
    };
    let access = super::AccessTokenCacheEntry {
        token: "private-access".to_string(),
    };
    for value in [
        &state as &dyn std::fmt::Debug,
        &session,
        &response,
        &introspection,
        &access,
    ] {
        assert!(!format!("{value:?}").contains("private-"));
        assert!(!format!("{value:#?}").contains("private-"));
        assert!(!format!("{:?}", Some(value)).contains("private-"));
    }
    assert_eq!(
        serde_json::to_value(&state).unwrap()["code_verifier"],
        "private-verifier"
    );
    assert_eq!(
        serde_json::to_value(&session).unwrap()["refresh_token"],
        "private-refresh"
    );
    assert_eq!(response.access_token, "private-access");
}

fn base_config() -> OpenidConnectConfig {
    OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        ..Default::default()
    }
}

fn plugin_log_entries(plugin_log: &PluginLog) -> Vec<String> {
    serde_json::to_value(plugin_log)
        .expect("plugin log must serialize")
        .get("log")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Shared Cookie header for `get_cookie` and `strip_cookie` on a mock session.
fn install_request_cookies(session: &mut MockPluginSession, cookie_header: &str) -> Arc<Mutex<Option<String>>> {
    let cookie = Arc::new(Mutex::new(
        (!cookie_header.is_empty()).then(|| cookie_header.to_string()),
    ));
    let for_get = Arc::clone(&cookie);
    session.expect_get_cookie().returning(move |name| {
        let header = for_get.lock().clone()?;
        header.split(';').find_map(|pair| {
            let mut parts = pair.trim().splitn(2, '=');
            let key = parts.next()?;
            (key == name).then(|| parts.next().unwrap_or("").to_string())
        })
    });
    // The request-header tiers are static: `strip_cookies` reads the Cookie
    // lines exactly once, so there is no need to model a live, rewritten map
    // (which mockall's reference return could not express anyway). The `Arc`
    // above still tracks what the rewrite produced.
    if cookie_header.is_empty() {
        setup_header_mocks(session, &[]);
    } else {
        setup_header_mocks(session, &[("cookie", cookie_header)]);
    }
    let for_set = Arc::clone(&cookie);
    session.expect_set_request_header_value().returning(move |name, value| {
        assert!(value.is_sensitive());
        if name.eq_ignore_ascii_case("Cookie") {
            *for_set.lock() = Some(value.to_str().unwrap().to_string());
        }
        Ok(())
    });
    let for_remove = Arc::clone(&cookie);
    session.expect_remove_request_header().returning(move |name| {
        if name.eq_ignore_ascii_case("Cookie") {
            *for_remove.lock() = None;
        }
        Ok(())
    });
    cookie
}

fn secret_ref(name: &str) -> edgion_resources::resources::common::SecretObjectReference {
    edgion_resources::resources::common::SecretObjectReference {
        group: None,
        kind: None,
        name: name.to_string(),
        namespace: None,
    }
}

fn oidc_secret(client_secret: &[u8], session_secret: &[u8]) -> k8s_openapi::api::core::v1::Secret {
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    k8s_openapi::api::core::v1::Secret {
        data: Some(BTreeMap::from([
            ("clientSecret".to_string(), ByteString(client_secret.to_vec())),
            ("sessionSecret".to_string(), ByteString(session_secret.to_vec())),
        ])),
        ..Default::default()
    }
}

#[test]
fn prepared_generation_clears_carriers_without_mutating_caller_and_uses_static_name() {
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::CustomCACertificateRef;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let identity =
        rcgen::generate_simple_self_signed(vec!["client.example.com".to_string()]).expect("generate test identity");
    let certificate = identity.cert.pem();
    let private_key = identity.signing_key.serialize_pem();
    let mut config = base_config();
    config.resolved_oidc_client_secret = Some("client-generation-one".to_string());
    config.resolved_session_secret = Some("session-generation-one-secret-32-bytes".to_string());
    config.tls.validation = Some(OutboundTlsValidation {
        ca_certificate_refs: Some(vec![CustomCACertificateRef {
            group: Some(String::new()),
            kind: "Secret".to_string(),
            name: "idp-ca".to_string(),
            namespace: None,
        }]),
        ..Default::default()
    });
    config.tls.client_certificate_ref = Some(secret_ref("idp-client"));
    config.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("idp-ca".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "ca.crt".to_string(),
            ByteString(certificate.as_bytes().to_vec()),
        )])),
        ..Default::default()
    }]);
    config.resolved_client_certificate = Some(k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("idp-client".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            ("tls.crt".to_string(), ByteString(certificate.into_bytes())),
            ("tls.key".to_string(), ByteString(private_key.into_bytes())),
        ])),
        ..Default::default()
    });

    let first = OpenidConnect::new(&config, "default".to_string());
    let second = OpenidConnect::new(&base_config(), "default".to_string());

    assert_eq!(first.name(), "OpenidConnect");
    assert_eq!(first.name().as_ptr(), second.name().as_ptr());
    assert_eq!(first.client_secret(), Some("client-generation-one"));
    assert!(first.session_cipher().is_some());
    assert!(first.config.resolved_oidc_client_secret.is_none());
    assert!(first.config.resolved_session_secret.is_none());
    assert!(first.config.resolved_ca_certificates.is_none());
    assert!(first.config.resolved_client_certificate.is_none());

    assert_eq!(
        config.resolved_oidc_client_secret.as_deref(),
        Some("client-generation-one")
    );
    assert_eq!(
        config.resolved_session_secret.as_deref(),
        Some("session-generation-one-secret-32-bytes")
    );
    assert!(config.resolved_ca_certificates.is_some());
    assert!(config.resolved_client_certificate.is_some());
}

#[test]
fn legacy_secret_fallback_is_constructor_scoped_and_generation_isolated() {
    const CHILD_ENV: &str = "EDGION_OIDC_SECRET_FALLBACK_CHILD";
    const TEST_NAME: &str = "legacy_secret_fallback_is_constructor_scoped_and_generation_isolated";
    const TEST_MODULE: &str = "plugins::http::openid_connect::plugin::tests";

    if std::env::var(CHILD_ENV).as_deref() != Ok(TEST_NAME) {
        let exact_test_name = format!("{TEST_MODULE}::{TEST_NAME}");
        let output = std::process::Command::new(
            std::env::current_exe().expect("the Rust test harness must have a current executable"),
        )
        .args(["--exact", exact_test_name.as_str(), "--nocapture"])
        .env(CHILD_ENV, TEST_NAME)
        .output()
        .expect("the isolated OIDC Secret fallback test process must start");
        assert!(
            output.status.success(),
            "isolated OIDC Secret fallback test failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    use edgion_common::secret::get_global_secret_store;

    const CLIENT_K1: &str = "client-generation-one";
    const CLIENT_K2: &str = "client-generation-two";
    const SESSION_K1: &str = "session-generation-one-secret-32-bytes";
    const SESSION_K2: &str = "session-generation-two-secret-32-bytes";

    let store = get_global_secret_store();
    store.replace_all(HashMap::from([(
        "default/oidc-secret".to_string(),
        oidc_secret(CLIENT_K1.as_bytes(), SESSION_K1.as_bytes()),
    )]));

    let mut fallback_config = base_config();
    fallback_config.client_secret_ref = Some(secret_ref("oidc-secret"));
    fallback_config.session_secret_ref = Some(secret_ref("oidc-secret"));
    fallback_config.resolved_oidc_client_secret = Some(String::new());
    fallback_config.resolved_session_secret = None;
    let generation_one = OpenidConnect::new(&fallback_config, "default".to_string());
    assert_eq!(generation_one.client_secret(), Some(CLIENT_K1));

    store.replace_all(HashMap::from([(
        "default/oidc-secret".to_string(),
        oidc_secret(CLIENT_K2.as_bytes(), SESSION_K2.as_bytes()),
    )]));

    assert_eq!(generation_one.client_secret(), Some(CLIENT_K1));
    let generation_two = OpenidConnect::new(&fallback_config, "default".to_string());
    assert_eq!(generation_two.client_secret(), Some(CLIENT_K2));

    let payload = super::AuthorizationStateCookie {
        state: "generation-one".to_string(),
        original_url: "/".to_string(),
        code_verifier: None,
        nonce: None,
        created_at: 1,
    };
    let generation_one_cookie = OpenidConnect::encode_signed_cookie_payload(
        &payload,
        generation_one.session_cipher().expect("generation one cipher"),
    )
    .expect("generation one cookie");
    assert!(
        OpenidConnect::decode_signed_cookie_payload::<super::AuthorizationStateCookie>(
            &generation_one_cookie,
            generation_one.session_cipher().expect("generation one cipher")
        )
        .is_ok()
    );
    assert!(
        OpenidConnect::decode_signed_cookie_payload::<super::AuthorizationStateCookie>(
            &generation_one_cookie,
            generation_two.session_cipher().expect("generation two cipher")
        )
        .is_err()
    );

    let mut authoritative_config = fallback_config.clone();
    authoritative_config.resolved_oidc_client_secret = Some("resolved-client".to_string());
    authoritative_config.resolved_session_secret = Some("resolved-session-secret-at-least-32-bytes".to_string());
    let authoritative = OpenidConnect::new(&authoritative_config, "default".to_string());
    assert_eq!(authoritative.client_secret(), Some("resolved-client"));
    assert_eq!(
        authoritative_config.resolved_oidc_client_secret.as_deref(),
        Some("resolved-client")
    );
    assert!(authoritative_config.resolved_session_secret.is_some());

    for (client_bytes, session_bytes) in [
        (b"\xff".as_slice(), SESSION_K1.as_bytes()),
        (b"".as_slice(), b"".as_slice()),
        (CLIENT_K1.as_bytes(), b"short".as_slice()),
    ] {
        store.replace_all(HashMap::from([(
            "default/oidc-secret".to_string(),
            oidc_secret(client_bytes, session_bytes),
        )]));
        let unavailable = OpenidConnect::new(&fallback_config, "default".to_string());
        assert_eq!(
            unavailable.client_secret().is_some(),
            !client_bytes.is_empty() && client_bytes.is_ascii()
        );
        assert_eq!(unavailable.session_cipher().is_some(), session_bytes.len() >= 32);
        assert_eq!(OpenidConnect::credential_failure().http_status(), 401);
        assert_eq!(OpenidConnect::credential_failure().user_message(), "Unauthorized");
    }

    store.replace_all(HashMap::new());
    let missing = OpenidConnect::new(&fallback_config, "default".to_string());
    assert!(missing.client_secret().is_none());
    assert!(missing.session_cipher().is_none());
}

#[tokio::test(start_paused = true)]
async fn unavailable_session_credential_returns_delayed_generic_401_with_static_log() {
    use mockall::Sequence;

    let mut config = base_config();
    config.bearer_only = false;
    config.unauth_action = UnauthAction::Auth;
    config.auth_failure_delay_ms = 100;
    config.resolved_oidc_client_secret = Some("client-secret-that-must-not-be-logged".to_string());
    config.resolved_session_secret = Some("short".to_string());
    let plugin = OpenidConnect::new(&config, "sensitive-namespace".to_string());

    let start = tokio::time::Instant::now();
    let mut sequence = Sequence::new();
    let mut session = MockPluginSession::new();
    session
        .expect_get_path()
        .times(2)
        .return_const("/.edgion/oidc/callback".to_string());
    session.expect_get_query_param().times(2).returning(|name| match name {
        "code" => Some("code".to_string()),
        "state" => Some("state".to_string()),
        _ => None,
    });
    session
        .expect_write_response_header()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |response, end| {
            assert_eq!(
                tokio::time::Instant::now().duration_since(start),
                Duration::from_millis(100)
            );
            assert!(!end);
            assert_eq!(response.status.as_u16(), 401);
            assert_eq!(
                response
                    .headers
                    .get("Content-Type")
                    .and_then(|value| value.to_str().ok()),
                Some("text/plain")
            );
            assert_eq!(
                response
                    .headers
                    .get("WWW-Authenticate")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer realm=\"edgion\"")
            );
            Ok(())
        });
    session
        .expect_write_response_body()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |body, end| {
            assert_eq!(
                tokio::time::Instant::now().duration_since(start),
                Duration::from_millis(100)
            );
            assert_eq!(body.as_deref(), Some(b"401 Unauthorized".as_slice()));
            assert!(end);
            Ok(())
        });
    session
        .expect_shutdown()
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|| {});

    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert!(log.contains("err: session credential unavailable"));
    assert!(!log.contains("sensitive-namespace"));
    assert!(!log.contains("client-secret-that-must-not-be-logged"));
}

fn selection_jwks(keys: serde_json::Value) -> JwkSet {
    serde_json::from_value(json!({ "keys": keys })).expect("valid JWKS fixture")
}

#[test]
fn select_jwk_with_kid_borrows_the_exact_set_entry() {
    let jwks = selection_jwks(json!([
        {"kty":"oct","kid":"first","k":"Zmlyc3Q","alg":"HS256"},
        {"kty":"oct","kid":"selected","k":"c2VsZWN0ZWQ","alg":"HS256"}
    ]));
    let header = Header {
        kid: Some("selected".to_string()),
        alg: Algorithm::HS256,
        ..Header::default()
    };

    let selected = select_jwk(&header, &jwks, Algorithm::HS256).expect("select by kid");

    assert!(std::ptr::eq(selected, &jwks.keys[1]));
}

#[test]
fn select_jwk_without_kid_borrows_the_only_compatible_entry() {
    let jwks = selection_jwks(json!([
        {"kty":"oct","kid":"wrong-before","k":"YmVmb3Jl","alg":"HS512"},
        {"kty":"oct","kid":"selected","k":"c2VsZWN0ZWQ","alg":"HS256"},
        {"kty":"oct","kid":"wrong-after","k":"YWZ0ZXI","alg":"HS384"}
    ]));
    let header = Header::new(Algorithm::HS256);

    let selected = select_jwk(&header, &jwks, Algorithm::HS256).expect("select compatible key");

    assert!(std::ptr::eq(selected, &jwks.keys[1]));
}

#[test]
fn select_jwk_without_kid_keeps_first_compatible_iteration_order() {
    let jwks = selection_jwks(json!([
        {"kty":"oct","kid":"first","k":"Zmlyc3Q","alg":"HS256"},
        {"kty":"oct","kid":"second","k":"c2Vjb25k","alg":"HS256"}
    ]));
    let header = Header::new(Algorithm::HS256);

    let selected = select_jwk(&header, &jwks, Algorithm::HS256).expect("select first compatible key");

    assert!(std::ptr::eq(selected, &jwks.keys[0]));
}

#[test]
fn select_jwk_kid_algorithm_mismatch_does_not_scan_later_duplicate_kid() {
    let jwks = selection_jwks(json!([
        {"kty":"oct","kid":"duplicate","k":"Zmlyc3Q","alg":"HS512"},
        {"kty":"oct","kid":"duplicate","k":"c2Vjb25k","alg":"HS256"}
    ]));
    let header = Header {
        kid: Some("duplicate".to_string()),
        alg: Algorithm::HS256,
        ..Header::default()
    };

    assert_eq!(
        select_jwk(&header, &jwks, Algorithm::HS256),
        Err(JwkSelectError::NoSuitableKey)
    );
}

#[test]
fn request_host_borrows_trimmed_first_host_value() {
    use crate::plugins::runtime::PluginSessionHeaderExt;

    for (raw, expected) in [
        (Some("  api.example.com , ignored.example"), Some("api.example.com")),
        (Some("api.example.com"), Some("api.example.com")),
        (Some(""), None),
        (Some(" \t"), None),
        (Some(",ignored.example"), None),
        (None, None),
    ] {
        let mut session = MockPluginSession::new();
        if let Some(raw) = raw {
            setup_header_mocks(&mut session, &[("host", raw)]);
        } else {
            setup_header_mocks(&mut session, &[]);
        }
        let host = OpenidConnect::request_host(&session);
        assert_eq!(host, expected);
        if let Some(host) = host {
            let raw = session.header_str("host").expect("host header");
            assert_eq!(host.as_ptr(), raw.trim_start().as_ptr());
        }
    }
}

#[test]
fn path_redirect_uris_ignore_x_forwarded_host() {
    let mut cfg = base_config();
    cfg.redirect_uri = Some("/oidc/callback".to_string());
    cfg.post_logout_redirect_uri = Some("/after-logout".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    setup_header_mocks(
        &mut session,
        &[("host", "api.example.com"), ("x-forwarded-host", "attacker.example")],
    );
    session.expect_downstream_is_tls().returning(|| Some(true));

    assert_eq!(
        plugin.resolve_redirect_uri(&session).expect("resolve redirect URI"),
        "https://api.example.com/oidc/callback"
    );
    assert_eq!(
        plugin.resolve_post_logout_redirect_uri_for_idp(&session),
        Some("https://api.example.com/after-logout".to_string())
    );
}

#[test]
fn path_redirect_uris_ignore_x_forwarded_proto() {
    let mut cfg = base_config();
    cfg.redirect_uri = Some("/oidc/callback".to_string());
    cfg.post_logout_redirect_uri = Some("/after-logout".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    // Hostile downgrade attempt on a TLS listener: the scheme must come from
    // the connection state, so the callback advertised to the IdP stays https.
    setup_header_mocks(
        &mut session,
        &[("host", "api.example.com"), ("x-forwarded-proto", "http")],
    );
    session.expect_downstream_is_tls().returning(|| Some(true));

    assert_eq!(
        plugin.resolve_redirect_uri(&session).expect("resolve redirect URI"),
        "https://api.example.com/oidc/callback"
    );
    assert_eq!(
        plugin.resolve_post_logout_redirect_uri_for_idp(&session),
        Some("https://api.example.com/after-logout".to_string())
    );
}

#[test]
fn path_redirect_uris_fall_back_to_https_when_tls_state_unknown() {
    let mut cfg = base_config();
    cfg.redirect_uri = Some("/oidc/callback".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    setup_header_mocks(
        &mut session,
        &[("host", "api.example.com"), ("x-forwarded-proto", "http")],
    );
    session.expect_downstream_is_tls().returning(|| None);

    assert_eq!(
        plugin.resolve_redirect_uri(&session).expect("resolve redirect URI"),
        "https://api.example.com/oidc/callback"
    );
}

#[test]
fn path_redirect_uris_use_http_on_a_plaintext_listener() {
    let mut cfg = base_config();
    cfg.redirect_uri = Some("/oidc/callback".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    // A forged `X-Forwarded-Proto: https` cannot upgrade the advertised scheme
    // either; deployments terminating TLS upstream must configure an absolute URL.
    setup_header_mocks(
        &mut session,
        &[("host", "api.example.com"), ("x-forwarded-proto", "https")],
    );
    session.expect_downstream_is_tls().returning(|| Some(false));

    assert_eq!(
        plugin.resolve_redirect_uri(&session).expect("resolve redirect URI"),
        "http://api.example.com/oidc/callback"
    );
}

#[test]
fn sanitize_redirect_target_rejects_cross_origin_forms() {
    let normalized = normalize_path("/%5Cevil.example/path", &PathNormalizationConfig::default())
        .expect("encoded backslash path must normalize");
    assert_eq!(normalized, r"/\evil.example/path");
    assert_eq!(OpenidConnect::sanitize_redirect_target(&normalized), "/");

    for target in [
        r"/\evil.example/path",
        r"/\\evil.example/path",
        "//evil.example/path",
        "https://evil.example/path",
        "",
        "orders?x=1",
    ] {
        assert_eq!(OpenidConnect::sanitize_redirect_target(target), "/");
    }
    let original_url = String::from("/orders?x=1");
    let target = OpenidConnect::sanitize_redirect_target(&original_url);
    assert_eq!(target, "/orders?x=1");
    assert_eq!(target.as_ptr(), original_url.as_ptr());
}

#[test]
fn test_introspection_cache_entry_has_no_expires_at() {
    // Compile-time check: IntrospectionCacheEntry must not have expires_at
    let _entry = IntrospectionCacheEntry {
        claims: serde_json::json!({"sub": "test"}),
        // If expires_at still exists, this will fail to compile with "missing field"
    };
}

#[tokio::test]
async fn test_bearer_only_without_token_returns_401() {
    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    setup_header_mocks(&mut session, &[]);
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
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    let cookies = install_request_cookies(&mut session, "");

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(cookies.lock().is_none());
}

#[tokio::test]
async fn test_unauth_action_auth_without_token_redirects_to_idp() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/orders".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
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
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/orders".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
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

#[test]
fn reserved_authorization_parameters_match_ascii_case_only() {
    for name in [
        "response_type",
        "client_id",
        "redirect_uri",
        "scope",
        "state",
        "nonce",
        "code_challenge",
        "code_challenge_method",
    ] {
        assert!(OpenidConnect::is_reserved_authorization_param(name));
        assert!(OpenidConnect::is_reserved_authorization_param(
            &name.to_ascii_uppercase()
        ));
        for (index, byte) in name.bytes().enumerate() {
            if byte.is_ascii_lowercase() {
                let mut mixed = name.as_bytes().to_vec();
                mixed[index] = byte.to_ascii_uppercase();
                let mixed = String::from_utf8(mixed).expect("ASCII parameter");
                assert!(OpenidConnect::is_reserved_authorization_param(&mixed));
            }
        }
        for candidate in [
            format!(" {name}"),
            format!("{name} "),
            format!("{name}x"),
            format!("x{name}"),
        ] {
            assert!(!OpenidConnect::is_reserved_authorization_param(&candidate));
        }
    }
    for name in ["", "prompt", "login_hint", "scopé", "π", "code-challenge", "nonce\0"] {
        assert!(!OpenidConnect::is_reserved_authorization_param(name));
    }
}

#[tokio::test]
async fn test_authorization_params_appends_non_reserved_params() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.authorization_params = Some(HashMap::from([
        ("foo".to_string(), "bar".to_string()),
        ("prompt".to_string(), "login".to_string()),
    ]));

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/orders".to_string());
    session.expect_get_query_param().returning(|_| None);
    session.expect_get_cookie().returning(|_| None);
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
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

async fn assert_invalid_callback_rejected(code: Option<&str>, state: Option<&str>, attacker_values: &[&str]) {
    const CALLBACK_QUERY_FIELDS: [&str; 5] = ["code", "state", "error", "error_description", "error_uri"];

    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session
        .expect_get_path()
        .times(2)
        .return_const("/.edgion/oidc/callback".to_string());
    let code = code.map(str::to_string);
    let state = state.map(str::to_string);
    session
        .expect_get_query_param()
        .times(2)
        .returning(move |name| match name {
            "code" => code.clone(),
            "state" => state.clone(),
            _ => panic!("invalid callback handling must not inspect query parameter {name}"),
        });
    let forbidden_values: Vec<String> = attacker_values
        .iter()
        .copied()
        .chain(CALLBACK_QUERY_FIELDS)
        .map(str::to_string)
        .collect();
    let header_forbidden = forbidden_values.clone();
    session
        .expect_write_response_header()
        .times(1)
        .returning(move |response, end| {
            assert_eq!(response.status.as_u16(), 400);
            assert!(!end);
            assert!(response.headers.get("Location").is_none());
            let rendered = format!("{response:?}");
            for value in &header_forbidden {
                assert!(!rendered.contains(value));
            }
            Ok(())
        });
    let body_forbidden = forbidden_values.clone();
    session
        .expect_write_response_body()
        .times(1)
        .returning(move |body, end| {
            assert_eq!(body.as_deref(), Some(b"400 Invalid OIDC callback".as_slice()));
            assert!(end);
            let rendered = String::from_utf8_lossy(body.as_deref().unwrap_or_default());
            for value in &body_forbidden {
                assert!(!rendered.contains(value));
            }
            Ok(())
        });
    session.expect_shutdown().times(1).returning(|| {});

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert_eq!(plugin_log_entries(&log), vec!["err: invalid callback"]);
    let serialized_log = serde_json::to_string(&log).expect("plugin log must serialize");
    for value in &forbidden_values {
        assert!(!serialized_log.contains(value));
    }
}

#[tokio::test]
async fn test_invalid_callback_forms_return_fixed_400_without_authentication_fallback() {
    for (code, state, attacker_values) in [
        (
            None,
            None,
            &["access_denied", "provider-error-detail", "provider-error-uri"][..],
        ),
        (Some(""), Some("attacker-state"), &["attacker-state"][..]),
        (Some("attacker-code"), Some(""), &["attacker-code"][..]),
        (Some("attacker-code"), None, &["attacker-code"][..]),
    ] {
        assert_invalid_callback_rejected(code, state, attacker_values).await;
    }
}

#[tokio::test]
async fn test_logout_path_clears_cookies_and_redirects() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    session.expect_get_cookie().returning(|_| None);
    setup_header_mocks(&mut session, &[]);
    session.expect_downstream_is_tls().returning(|| Some(true));
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
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/after-logout".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: Some("https://idp.example.com/logout".to_string()),
            revocation_endpoint: None,
        })
        .await;

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    session.expect_get_cookie().returning(|_| None);
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
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
    cfg.revoke_tokens_on_logout = true;
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/after-logout".to_string());

    let plugin = Arc::new(OpenidConnect::new(&cfg, "default".to_string()));
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: Some("https://idp.example.com/logout".to_string()),
            revocation_endpoint: Some("https://idp.example.com/revoke".to_string()),
        })
        .await;

    let now = OpenidConnect::now_unix_secs();
    let session_cookie_payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: String::new(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: Some("id-token-hint".to_string()),
        refresh_token: Some("refresh-token".to_string()),
    };
    let encoded_cookie = OpenidConnect::encode_signed_cookie_payload(
        &session_cookie_payload,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("encode session cookie");
    plugin
        .cache_access_token("session-ref", "access-token", Some(now + 60))
        .await;

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
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
    let plugin_at_response = plugin.clone();
    session.expect_write_response_header().returning(move |resp, end| {
        assert!(end);
        assert!(plugin_at_response.access_token_cache.get("session-ref").is_none());
        assert_eq!(resp.status.as_u16(), 302);
        let location = resp.headers.get("Location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(location.contains("post_logout_redirect_uri=https%3A%2F%2Fapi.example.com%2Fafter-logout"));
        assert!(location.contains("id_token_hint=id-token-hint"));
        let cookies: Vec<&str> = resp
            .headers
            .get_all("Set-Cookie")
            .iter()
            .map(|value| value.to_str().expect("set-cookie must be ASCII"))
            .collect();
        assert_eq!(
            cookies,
            vec![
                "edgion_oidc_session=; Path=/; SameSite=Lax; Max-Age=0; HttpOnly; Secure",
                "edgion_oidc_state=; Path=/.edgion/oidc/callback; SameSite=Lax; Max-Age=0; HttpOnly; Secure",
            ]
        );
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(plugin.access_token_cache.get("session-ref").is_none());
    assert_eq!(
        plugin_log_entries(&log),
        vec!["err: revocation credential unavailable", "logout handled"]
    );
}

#[tokio::test]
async fn test_logout_with_revoke_enabled_and_no_session_cookie_still_redirects() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.revoke_tokens_on_logout = true;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg.logout_path = "/auth/logout".to_string();
    cfg.post_logout_redirect_uri = Some("/".to_string());

    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    session.expect_get_cookie().returning(|_| None);
    setup_header_mocks(&mut session, &[]);
    session.expect_downstream_is_tls().returning(|| Some(true));
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
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let payload = super::AuthorizationStateCookie {
        state: "abc".to_string(),
        original_url: "/orders".to_string(),
        code_verifier: Some("verifier".to_string()),
        nonce: Some("nonce-value".to_string()),
        created_at: 123,
    };
    let encoded = OpenidConnect::encode_signed_cookie_payload(
        &payload,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("encode");
    let decoded: super::AuthorizationStateCookie = OpenidConnect::decode_signed_cookie_payload(
        &encoded,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("decode");
    assert_eq!(decoded.state, "abc");
    assert_eq!(decoded.original_url, "/orders");
}

#[tokio::test]
async fn test_maybe_set_session_cookie_header_strips_access_token_and_caches_it() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Auth;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
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

    let captured_cookie = Arc::new(Mutex::new(None::<String>));
    let captured_cookie_clone = captured_cookie.clone();

    let mut session = MockPluginSession::new();
    session.expect_append_response_header().returning(move |name, value| {
        if name.eq_ignore_ascii_case("Set-Cookie") {
            *captured_cookie_clone.lock() = Some(value.to_string());
        }
        Ok(())
    });

    plugin.maybe_set_session_cookie_header(&mut session, &payload).await;

    let set_cookie = captured_cookie.lock().clone().expect("set-cookie header");
    let encoded = set_cookie
        .strip_prefix("edgion_oidc_session=")
        .and_then(|v| v.split(';').next())
        .unwrap_or("");
    let decoded: OidcSessionCookie =
        OpenidConnect::decode_signed_cookie_payload(encoded, plugin.session_cipher().expect("prepared session cipher"))
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
    cfg.renew_access_token_on_expiry = true;
    cfg.access_token_expires_leeway = 30;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: Some("https://idp.example.com/introspect".to_string()),
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

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
        refresh_token: Some("refresh-token".to_string()),
    };
    let encoded_cookie = OpenidConnect::encode_signed_cookie_payload(
        &session_cookie_payload,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("encode session cookie");
    let lock_key = OpenidConnect::refresh_lock_key(&encoded_cookie);
    plugin
        .put_refresh_result(
            &lock_key,
            OidcSessionCookie {
                session_ref: "session-ref".to_string(),
                access_token: token.to_string(),
                created_at: now,
                expires_at: Some(now + 60),
                id_token: None,
                refresh_token: Some("refresh-token".to_string()),
            },
        )
        .await;

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    let cookies = install_request_cookies(
        &mut session,
        &format!("app=keep; edgion_oidc_session={encoded_cookie}; theme=dark"),
    );
    session.expect_append_response_header().times(0);

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(plugin_log_entries(&log), vec!["session token verified"]);
    assert_eq!(cookies.lock().as_deref(), Some("app=keep; theme=dark"));
}

#[tokio::test]
async fn test_run_request_cache_miss_uses_refresh_singleflight_cached_result() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Deny;
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.introspection_cache_ttl = 60;
    cfg.renew_access_token_on_expiry = true;
    cfg.access_token_expires_leeway = 30;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: Some("https://idp.example.com/introspect".to_string()),
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

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
    let encoded_cookie = OpenidConnect::encode_signed_cookie_payload(
        &session_cookie_payload,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("encode session cookie");
    let lock_key = OpenidConnect::refresh_lock_key(&encoded_cookie);
    plugin
        .put_refresh_result(
            &lock_key,
            OidcSessionCookie {
                session_ref: "session-ref".to_string(),
                access_token: refreshed_token.to_string(),
                created_at: now,
                expires_at: Some(now + 1),
                id_token: None,
                refresh_token: Some("refresh-token".to_string()),
            },
        )
        .await;

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    let cookies = install_request_cookies(&mut session, &format!("edgion_oidc_session={encoded_cookie}"));
    session
        .expect_append_response_header()
        .withf(|name, _| name.eq_ignore_ascii_case("Set-Cookie"))
        .times(1)
        .returning(|_, _| Ok(()));

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(
        plugin_log_entries(&log),
        vec!["token refresh miss", "session token verified"]
    );
    assert!(cookies.lock().is_none());
}

#[tokio::test(start_paused = true)]
async fn test_run_request_cache_miss_waits_only_one_singleflight_timeout() {
    let mut cfg = base_config();
    cfg.timeout = 1;
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Pass;
    cfg.renew_access_token_on_expiry = true;
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = Arc::new(OpenidConnect::new(&cfg, "default".to_string()));

    let now = OpenidConnect::now_unix_secs();
    let payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: String::new(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };
    let encoded_cookie = OpenidConnect::encode_signed_cookie_payload(
        &payload,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("encode session cookie");
    let lock_key = OpenidConnect::refresh_lock_key(&encoded_cookie);
    let flight = plugin.refresh_singleflight.get_or_create(&lock_key);
    let held_guard = flight.lock().lock().await;

    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    let cookies = install_request_cookies(
        &mut session,
        &format!("edgion_oidc_session={encoded_cookie}; edgion_oidc_state=pending"),
    );

    let plugin_for_request = plugin.clone();
    let handle = tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        let mut log = PluginLog::new("OpenidConnect");
        let result = plugin_for_request.run_request(&mut session, &mut log).await;
        (result, log, tokio::time::Instant::now().duration_since(start))
    });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(999)).await;
    tokio::task::yield_now().await;
    assert!(
        !handle.is_finished(),
        "the one-second singleflight wait must still be active"
    );

    tokio::time::advance(Duration::from_millis(1)).await;
    for _ in 0..16 {
        if handle.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(handle.is_finished(), "a second preemptive wait must not start");
    let (result, log, elapsed) = handle.await.expect("request task must complete");
    drop(held_guard);

    assert_eq!(elapsed, Duration::from_secs(1));
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(
        plugin_log_entries(&log),
        vec!["err: refresh wait timeout", "err: session no token", "no token; pass"]
    );
    assert!(cookies.lock().is_none());
}

#[tokio::test]
async fn test_run_request_empty_refresh_token_with_unavailable_secret_attempts_once() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Pass;
    cfg.renew_access_token_on_expiry = true;
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: None,
            token_endpoint: Some("https://idp.example.com/token".to_string()),
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    let now = OpenidConnect::now_unix_secs();
    let payload = OidcSessionCookie {
        session_ref: "session-ref".to_string(),
        access_token: String::new(),
        created_at: now,
        expires_at: Some(now + 60),
        id_token: None,
        refresh_token: Some(String::new()),
    };
    let encoded_cookie = OpenidConnect::encode_signed_cookie_payload(
        &payload,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("encode session cookie");

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");
    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    let cookies = install_request_cookies(&mut session, &format!("edgion_oidc_session={encoded_cookie}"));

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(
        plugin_log_entries(&log),
        vec!["err: token refresh fail", "err: session no token", "no token; pass"]
    );
    assert!(cookies.lock().is_none());
}

#[tokio::test(start_paused = true)]
async fn session_refresh_timeout_dye_reaches_jwks_without_bearer_jwks_or_nonce() {
    use crate::plugins::runtime::PluginRuntime;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugin, RequestFilterEntry};

    for mode in [VerificationMode::IntrospectionOnly, VerificationMode::Auto] {
        for stale_cache in [false, true] {
            let mut cfg = base_config();
            cfg.verification_mode = mode;
            cfg.use_jwks = false;
            cfg.use_nonce = false;
            cfg.bearer_only = false;
            cfg.renew_access_token_on_expiry = true;
            cfg.timeout = 1;
            cfg.introspection_cache_ttl = 60;
            cfg.clock_skew_seconds = 0;
            cfg.access_token_expires_leeway = 0;
            cfg.token_signing_alg = Some("HS256".to_string());
            cfg.allowed_signing_algs = Some(vec!["HS256".to_string()]);
            cfg.unauth_action = UnauthAction::Pass;
            cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
            cfg.resolved_oidc_client_secret = Some("test-client-secret".to_string());

            let mut entry = RequestFilterEntry::new(EdgionPlugin::OpenidConnect(Box::new(cfg.clone())));
            entry.dye = Some(
                serde_json::from_value(json!({
                    "response": [{"name": "X-OIDC-Degraded", "on": ["fail-open", "fail-close"]}]
                }))
                .unwrap(),
            );
            assert_eq!(entry.validate_dye(), None);
            let mut owner: edgion_resources::EdgionPlugins = serde_json::from_value(json!({
                "metadata": {"name": "oidc-dye", "namespace": "default"},
                "spec": {}
            }))
            .unwrap();
            owner.spec.request_plugins = Some(vec![entry]);
            let mut runtime = PluginRuntime::new();
            assert!(runtime
                .add_resource_filters_with_cache(
                    &owner,
                    &mut crate::plugins::runtime::entry_bundle::EntryCache::default(),
                )
                .is_empty());
            let plan = runtime
                .request_plugins_iter()
                .next()
                .unwrap()
                .dye_plan()
                .unwrap()
                .clone();
            assert_eq!(plan.response.len(), 1);

            let plugin = Arc::new(OpenidConnect::new(&cfg, "default".to_string()));
            // The existing egress policy rejects this JWKS destination before dialing.
            plugin
                .jwks_provider
                .seed_discovery(DiscoveryDocument {
                    issuer: "https://idp.example.com".to_string(),
                    jwks_uri: "http://169.254.169.254/jwks".to_string(),
                    authorization_endpoint: None,
                    token_endpoint: None,
                    introspection_endpoint: Some("http://169.254.169.254/introspect".to_string()),
                    userinfo_endpoint: None,
                    end_session_endpoint: None,
                    revocation_endpoint: None,
                })
                .await;
            if stale_cache {
                plugin
                    .jwks_provider
                    .seed_stale_jwks(
                        serde_json::from_value(json!({
                            "keys": [{"kty": "oct", "k": "c2VjcmV0", "alg": "HS256"}]
                        }))
                        .unwrap(),
                    )
                    .await;
            }

            let now = OpenidConnect::now_unix_secs();
            let claims = json!({"active": true, "iss": "https://idp.example.com", "exp": now - 120});
            let token = jsonwebtoken::encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &jsonwebtoken::EncodingKey::from_secret(b"secret"),
            )
            .unwrap();
            // Seed the exact expired introspection result consumed by the verifier,
            // bypassing network I/O and normal cache TTL clipping at token expiry.
            plugin.introspection_cache.insert(
                OpenidConnect::introspection_cache_key(&token),
                IntrospectionCacheEntry { claims },
                Duration::from_secs(60),
            );
            let payload = OidcSessionCookie {
                session_ref: "dye-session".to_string(),
                access_token: token,
                created_at: now,
                expires_at: Some(now + 3600),
                id_token: None,
                refresh_token: Some("refresh-token".to_string()),
            };
            let cookie =
                OpenidConnect::encode_signed_cookie_payload(&payload, plugin.session_cipher().unwrap()).unwrap();
            let flight = plugin
                .refresh_singleflight
                .get_or_create(&OpenidConnect::refresh_lock_key(&cookie));
            let held_guard = flight.lock().lock().await;
            let mut session = MockPluginSession::new();
            session.expect_get_path().return_const("/".to_string());
            session.expect_get_query_param().returning(|_| None);
            let cookies = install_request_cookies(&mut session, &format!("edgion_oidc_session={cookie}"));
            let expected = if stale_cache {
                DyeOutcome::FailOpen
            } else {
                DyeOutcome::FailClose
            };
            let recorded = Arc::new(Mutex::new(Vec::new()));
            let records = recorded.clone();
            session
                .expect_record_outcome()
                .times(1)
                .returning(move |outcome, value| {
                    assert_eq!(outcome, expected);
                    assert_eq!(
                        value,
                        if stale_cache {
                            "jwks-stale-cache"
                        } else {
                            "jwks-unreachable"
                        }
                    );
                    assert!(plan.response[0].matches_outcome(outcome));
                    records.lock().push(outcome);
                });
            let handle = tokio::spawn(async move {
                let start = tokio::time::Instant::now();
                let mut log = PluginLog::new("OpenidConnect");
                let result = plugin.run_request(&mut session, &mut log).await;
                (result, log, start.elapsed())
            });
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_millis(999)).await;
            tokio::task::yield_now().await;
            assert!(!handle.is_finished());
            assert!(recorded.lock().is_empty());
            tokio::time::advance(Duration::from_millis(1)).await;
            let (result, log, elapsed) = handle.await.unwrap();
            drop(held_guard);
            assert_eq!(elapsed, Duration::from_secs(1));
            assert_eq!(*recorded.lock(), vec![expected]);
            // Even zero leeway reaches JWKS before rejecting stale-token reuse.
            assert_eq!(result, PluginRunningResult::GoodNext);
            assert_eq!(
                plugin_log_entries(&log),
                vec!["err: refresh wait timeout", "no token; pass"]
            );
            assert!(cookies.lock().is_none());
        }
    }
}

#[test]
fn strip_gateway_owned_cookies_removes_session_and_state_keeps_others() {
    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let mut session = MockPluginSession::new();
    let cookies = install_request_cookies(
        &mut session,
        "app=keep; edgion_oidc_session=secret; edgion_oidc_state=state; theme=dark",
    );

    plugin.strip_gateway_owned_cookies(&mut session);

    assert_eq!(cookies.lock().as_deref(), Some("app=keep; theme=dark"));
}

#[test]
fn strip_gateway_owned_cookies_drops_header_when_only_gateway_cookies() {
    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let mut session = MockPluginSession::new();
    let cookies = install_request_cookies(&mut session, "edgion_oidc_session=secret; edgion_oidc_state=state");

    plugin.strip_gateway_owned_cookies(&mut session);

    assert!(cookies.lock().is_none());
}

#[test]
fn strip_gateway_owned_cookies_uses_configured_session_name() {
    let mut cfg = base_config();
    cfg.session_cookie_name = "custom_oidc_session".to_string();
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let cookies = install_request_cookies(
        &mut session,
        "custom_oidc_session=secret; edgion_oidc_session=app-owned; other=1",
    );

    plugin.strip_gateway_owned_cookies(&mut session);

    assert_eq!(
        cookies.lock().as_deref(),
        Some("edgion_oidc_session=app-owned; other=1")
    );
}

#[test]
fn strip_gateway_owned_cookies_merges_multiple_cookie_lines() {
    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let mut session = MockPluginSession::new();
    // Two separate Cookie field lines; one pass must merge them and rewrite once.
    let cookie: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    setup_header_mocks(
        &mut session,
        &[
            ("cookie", "edgion_oidc_session=secret; app=keep"),
            ("cookie", "edgion_oidc_state=state; theme=dark"),
        ],
    );
    let for_set = Arc::clone(&cookie);
    session
        .expect_set_request_header_value()
        .times(1)
        .returning(move |name, value| {
            assert!(name.eq_ignore_ascii_case("Cookie"));
            assert!(value.is_sensitive());
            *for_set.lock() = Some(value.to_str().unwrap().to_string());
            Ok(())
        });
    session.expect_remove_request_header().returning(|_| {
        panic!("unrelated cookies must keep the Cookie header");
    });

    plugin.strip_gateway_owned_cookies(&mut session);

    assert_eq!(cookie.lock().as_deref(), Some("app=keep; theme=dark"));
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
    assert!(OpenidConnect::should_auto_fallback_to_introspection(
        &super::OidcVerifyError::InvalidFormat
    ));
    assert!(!OpenidConnect::should_auto_fallback_to_introspection(
        &super::OidcVerifyError::Auth(super::AuthFailure::InvalidSignature)
    ));
    assert!(!OpenidConnect::should_auto_fallback_to_introspection(
        &super::OidcVerifyError::dependency(
            super::OidcOperation::Introspection,
            super::OidcDependencyFailure::Transport
        )
    ));
}

#[test]
fn test_is_refresh_wait_timeout() {
    assert!(OpenidConnect::is_refresh_wait_timeout(
        &super::OidcVerifyError::RefreshSingleflightTimeout
    ));
    assert!(!OpenidConnect::is_refresh_wait_timeout(
        &super::OidcVerifyError::dependency(
            super::OidcOperation::Introspection,
            super::OidcDependencyFailure::Transport
        )
    ));
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
fn test_validate_claims_from_value_returns_expired_when_exp_passed() {
    let mut cfg = base_config();
    cfg.clock_skew_seconds = 0;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let now = OpenidConnect::now_unix_secs();
    let claims = json!({ "exp": now.saturating_sub(10) });
    assert_eq!(
        plugin.validate_claims_from_value(&claims, None),
        Err(super::OidcVerifyError::Expired)
    );
}

#[test]
fn test_validate_claims_from_value_preserves_issuer_tristate() {
    let invalid_issuer = || super::OidcVerifyError::Auth(super::AuthFailure::InvalidIssuer);

    let mut configured = base_config();
    configured.issuers = Some(vec!["https://configured.example.com".to_string()]);
    let configured_plugin = OpenidConnect::new(&configured, "default".to_string());
    assert!(configured_plugin
        .validate_claims_from_value(
            &json!({"iss": "https://configured.example.com"}),
            Some("https://default.example.com"),
        )
        .is_ok());
    assert_eq!(
        configured_plugin.validate_claims_from_value(
            &json!({"iss": "https://default.example.com"}),
            Some("https://default.example.com"),
        ),
        Err(invalid_issuer())
    );

    let mut disabled = base_config();
    disabled.issuers = Some(Vec::new());
    let disabled_plugin = OpenidConnect::new(&disabled, "default".to_string());
    assert!(disabled_plugin
        .validate_claims_from_value(&json!({}), Some("https://default.example.com"))
        .is_ok());
    assert!(disabled_plugin
        .validate_claims_from_value(
            &json!({"iss": "https://mismatch.example.com"}),
            Some("https://default.example.com"),
        )
        .is_ok());

    let defaulted_plugin = OpenidConnect::new(&base_config(), "default".to_string());
    assert!(defaulted_plugin
        .validate_claims_from_value(
            &json!({"iss": "https://default.example.com"}),
            Some("https://default.example.com"),
        )
        .is_ok());
    assert_eq!(
        defaulted_plugin.validate_claims_from_value(
            &json!({"iss": "https://mismatch.example.com"}),
            Some("https://default.example.com"),
        ),
        Err(invalid_issuer())
    );
    assert_eq!(
        defaulted_plugin.validate_claims_from_value(&json!({}), Some("https://default.example.com")),
        Err(invalid_issuer())
    );
    assert!(defaulted_plugin.validate_claims_from_value(&json!({}), None).is_ok());
}

#[test]
fn test_validate_required_scopes_unions_scope_and_scp_forms() {
    let mut cfg = base_config();
    cfg.required_scopes = Some(vec!["openid".to_string(), "api:read".to_string(), "groups".to_string()]);
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    assert!(plugin
        .validate_required_scopes(&json!({
            "scope": "openid api:read openid",
            "scp": "groups api:read groups",
        }))
        .is_ok());
    assert!(plugin
        .validate_required_scopes(&json!({
            "scope": "openid openid",
            "scp": ["api:read", 7, "groups", null, "api:read"],
        }))
        .is_ok());
}

#[test]
fn test_validate_required_scopes_ignores_non_strings_and_reports_missing_scope() {
    let mut cfg = base_config();
    cfg.required_scopes = Some(vec!["api:read".to_string(), "api:write".to_string()]);
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let insufficient_scope = || super::OidcVerifyError::Auth(super::AuthFailure::InsufficientScope);

    for claims in [
        json!({"scope": 7, "scp": ["api:read", "api:read", false, null]}),
        json!({}),
    ] {
        assert_eq!(plugin.validate_required_scopes(&claims), Err(insufficient_scope()));
    }
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
        Err(super::OidcVerifyError::Auth(super::AuthFailure::InsufficientScope))
    );
}

#[test]
fn test_should_preemptive_refresh() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.renew_access_token_on_expiry = true;
    cfg.access_token_expires_leeway = 30;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
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
    let no_exp = OidcSessionCookie {
        expires_at: None,
        ..far_exp.clone()
    };
    let no_refresh = OidcSessionCookie {
        refresh_token: None,
        ..near_exp.clone()
    };
    let cookie_token_missing = OidcSessionCookie {
        access_token: String::new(),
        ..far_exp.clone()
    };
    assert!(plugin.should_preemptive_refresh("resolved-token", &near_exp));
    assert!(!plugin.should_preemptive_refresh("resolved-token", &far_exp));
    assert!(!plugin.should_preemptive_refresh("resolved-token", &cookie_token_missing));
    assert!(!plugin.should_preemptive_refresh("resolved-token", &no_exp));
    assert!(!plugin.should_preemptive_refresh("resolved-token", &no_refresh));
    assert!(plugin.should_preemptive_refresh("", &far_exp));

    let mut disabled_cfg = cfg;
    disabled_cfg.renew_access_token_on_expiry = false;
    let disabled = OpenidConnect::new(&disabled_cfg, "default".to_string());
    assert!(!disabled.should_preemptive_refresh("", &near_exp));
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
async fn inactive_introspection_result_is_never_cached() {
    let mut cfg = base_config();
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    plugin
        .cache_introspection_claims("inactive-token", &json!({"active": false, "sub": "u1"}))
        .await;

    assert_eq!(plugin.get_cached_introspection_claims("inactive-token").await, None);
}

#[test]
fn introspection_cache_ttl_never_exceeds_config_or_token_lifetime() {
    use std::time::Duration as StdDuration;

    let now = 1_000;
    assert_eq!(
        OpenidConnect::bounded_introspection_cache_ttl(&json!({"exp": now + 5}), 60, now),
        StdDuration::from_secs(5)
    );
    assert_eq!(
        OpenidConnect::bounded_introspection_cache_ttl(&json!({"exp": now + 120}), 60, now),
        StdDuration::from_secs(60)
    );
    assert_eq!(
        OpenidConnect::bounded_introspection_cache_ttl(&json!({}), 60, now),
        StdDuration::from_secs(60)
    );
    assert_eq!(
        OpenidConnect::bounded_introspection_cache_ttl(&json!({"exp": now}), 60, now),
        StdDuration::ZERO
    );
    assert_eq!(
        OpenidConnect::bounded_introspection_cache_ttl(&json!({"exp": now - 1}), 60, now),
        StdDuration::ZERO
    );
}

#[tokio::test]
async fn expired_introspection_result_remains_a_miss_after_another_insert() {
    use std::time::Duration as StdDuration;
    let mut cfg = base_config();
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let expired_key = OpenidConnect::introspection_cache_key("expired-token");
    // Insert with a 1ms TTL so it expires immediately
    plugin.introspection_cache.insert(
        expired_key.clone(),
        IntrospectionCacheEntry {
            claims: json!({"active": true, "sub": "old"}),
        },
        StdDuration::from_millis(1),
    );
    // Wait for it to expire
    tokio::time::sleep(StdDuration::from_millis(10)).await;

    plugin
        .cache_introspection_claims("fresh-token", &json!({"active": true, "sub": "new"}))
        .await;

    // Unchanged TinyUFO reclaims expired slots through normal eviction, but
    // the wrapper must keep the expired record logically invisible.
    assert!(plugin.introspection_cache.get(&expired_key).0.is_none());
}

#[tokio::test]
async fn test_verify_token_auto_falls_back_to_introspection_cache_on_invalid_jwt_format() {
    let mut cfg = base_config();
    cfg.verification_mode = VerificationMode::Auto;
    cfg.use_jwks = true;
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://issuer.example.com".to_string(),
            jwks_uri: "https://issuer.example.com/jwks".to_string(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: Some("https://issuer.example.com/introspect".to_string()),
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    let token = "bad.header.payload";
    let now = OpenidConnect::now_unix_secs();
    let claims = json!({
        "active": true,
        "iss": "https://issuer.example.com",
        "exp": now + 120
    });
    plugin.cache_introspection_claims(token, &claims).await;

    let mut session = MockPluginSession::new();
    let verified = plugin
        .verify_token(&mut session, token)
        .await
        .expect("verify via fallback");
    assert_eq!(verified, claims);
}

#[tokio::test]
async fn test_refresh_singleflight_wait_timeout() {
    let mut cfg = base_config();
    cfg.timeout = 1;
    cfg.bearer_only = false;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    let lock_key = "refresh-lock";
    let flight = plugin.refresh_singleflight.get_or_create(lock_key);
    let _held_guard = flight.lock().lock().await;

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
    assert_eq!(result, Err(super::OidcVerifyError::RefreshSingleflightTimeout));
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

    let flight = plugin.refresh_singleflight.get_or_create(lock_key);
    let _held_guard = flight.lock().lock().await;

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
async fn test_try_get_recent_refresh_result_returns_none_after_ttl() {
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
    // Force a short TTL so the entry expires before we read it back; LRU lazy-expiry
    // is what `try_get_recent_refresh_result` now relies on for the freshness window.
    // 100ms gives plenty of margin on a loaded CI runner without dragging out the suite.
    plugin
        .refresh_singleflight_results
        .insert(lock_key.to_string(), payload, Some(Duration::from_millis(10)));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(plugin.try_get_recent_refresh_result(lock_key).await.is_none());
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
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Access-Token" && value == "access-token" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-ID-Token" && value == "id-token" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "Authorization" && value == "Bearer access-token" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Userinfo" && value == "{\"sub\":\"userinfo-user\"}" && value.is_sensitive())
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

    session.expect_set_request_header_value().never();
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

    session.expect_set_request_header_value().never();
    plugin.apply_upstream_headers(
        &mut session,
        "access-token",
        None,
        Some("{\"sub\":\"user\n1\"}"),
        &json!({"sub": "user-1"}),
    );
}

#[test]
fn claim_projection_uses_canonical_header_name_and_failed_fixed_write_preserves_budget() {
    let mut cfg = base_config();
    cfg.set_access_token_header = true;
    cfg.claims_to_headers = Some(HashMap::from([("sub".to_string(), "X-Sub".to_string())]));
    cfg.max_header_value_bytes = 15;
    cfg.max_total_header_bytes = 15;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();
    let mut sequence = mockall::Sequence::new();

    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Access-Token" && value == "a" && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Err(std::io::Error::other("injected write failure").into()));
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "x-sub" && value == "u" && value.is_sensitive())
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));

    plugin.apply_upstream_headers(&mut session, "a", None, None, &json!({"sub": "u"}));
}

#[tokio::test]
async fn test_fetch_userinfo_json_returns_none_without_userinfo_endpoint() {
    let mut cfg = base_config();
    cfg.set_userinfo_header = true;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    assert_eq!(
        plugin
            .fetch_userinfo_json("access-token", &mut PluginLog::new("OpenidConnect"))
            .await,
        None
    );
}

#[test]
fn test_two_openid_instances_get_distinct_clients() {
    let a = OpenidConnect::new(&base_config(), "default".to_string());
    let b = OpenidConnect::new(&base_config(), "default".to_string());
    // Each instance builds its own reqwest::Client.
    assert!(!std::sync::Arc::ptr_eq(&a.client, &b.client));
    assert!(std::ptr::eq(a.http_client(), a.client.as_ref()));
    assert!(std::ptr::eq(b.http_client(), b.client.as_ref()));
}

#[tokio::test]
async fn test_remove_cached_access_token() {
    let cfg = base_config();
    let oidc = OpenidConnect::new(&cfg, "default".to_string());
    let now = OpenidConnect::now_unix_secs();

    oidc.cache_access_token("session-abc", "token-xyz", Some(now + 60))
        .await;
    assert_eq!(
        oidc.get_cached_access_token("session-abc").await,
        Some("token-xyz".to_string()),
        "token should be cached before removal"
    );

    oidc.remove_cached_access_token("session-abc").await;
    assert_eq!(
        oidc.get_cached_access_token("session-abc").await,
        None,
        "token should be gone after removal"
    );

    // No-op on empty string
    oidc.remove_cached_access_token("").await; // should not panic
}

// ── Group I: load-time path normalization ─────────────────────────────────────

#[test]
fn config_logout_path_normalized_at_load() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        logout_path: "/auth//logout".to_string(),
        ..Default::default()
    };
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    // Double-slash must be collapsed at construction time.
    assert_eq!(plugin.config.logout_path, "/auth/logout");
}

#[test]
fn config_logout_path_already_normalized_unchanged() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        logout_path: "/auth/logout".to_string(),
        ..Default::default()
    };
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    assert_eq!(plugin.config.logout_path, "/auth/logout");
}

#[test]
fn callback_path_normalized_for_path_only_redirect_uri() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        redirect_uri: Some("/auth//oidc//callback".to_string()),
        ..Default::default()
    };
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    // Double-slashes must be collapsed when the callback path is cached.
    assert_eq!(plugin.callback_path(), "/auth/oidc/callback");
}

#[test]
fn callback_path_normalized_for_absolute_url_redirect_uri() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        redirect_uri: Some("https://app.example.com/auth//callback".to_string()),
        ..Default::default()
    };
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    // Construction extracts the URL path and then normalizes it.
    assert_eq!(plugin.callback_path(), "/auth/callback");
}

#[test]
fn oidc_san_pinning_config_builds_with_system_ca() {
    use crate::tls::validation::build_verified_outbound_tls;
    use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};

    let resolved = ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::System,
        client_identity: None,
        san: None,
    };
    let san = vec![SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some("idp.example.com".to_string()),
        uri: None,
    }];
    let cfg = build_verified_outbound_tls(&resolved, san, "OpenidConnect").expect("build ok");
    assert_eq!(cfg.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
}

#[test]
fn oidc_san_pinning_config_fails_with_empty_custom_ca() {
    use crate::tls::validation::build_verified_outbound_tls;
    use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};

    let resolved = ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::Custom(vec![b"not-a-pem".to_vec()]),
        client_identity: None,
        san: None,
    };
    let san = vec![SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some("idp.example.com".to_string()),
        uri: None,
    }];
    let r = build_verified_outbound_tls(&resolved, san, "OpenidConnect");
    assert!(r.is_err(), "invalid CA PEM should error");
}

#[test]
fn oidc_construction_fails_closed_when_custom_ca_is_unresolved() {
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::CustomCACertificateRef;

    let mut config = base_config();
    config.tls.verify = Some(true);
    config.tls.validation = Some(OutboundTlsValidation {
        ca_certificate_refs: Some(vec![CustomCACertificateRef {
            group: Some(String::new()),
            kind: "Secret".to_string(),
            name: "idp-ca".to_string(),
            namespace: None,
        }]),
        ..Default::default()
    });

    let error = actual_oidc(&config, "default".to_string())
        .err()
        .expect("unresolved custom CA must reject plugin construction");
    assert_eq!(error, "openid_connect outbound HTTP client build failed");
}

#[test]
fn oidc_construction_fails_closed_when_custom_ca_contains_no_certificate() {
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::CustomCACertificateRef;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let mut config = base_config();
    config.tls.verify = Some(true);
    config.tls.validation = Some(OutboundTlsValidation {
        ca_certificate_refs: Some(vec![CustomCACertificateRef {
            group: Some(String::new()),
            kind: "Secret".to_string(),
            name: "idp-ca".to_string(),
            namespace: None,
        }]),
        ..Default::default()
    });
    config.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("idp-ca".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "ca.crt".to_string(),
            ByteString(b"not-a-pem".to_vec()),
        )])),
        ..Default::default()
    }]);

    let error = actual_oidc(&config, "default".to_string())
        .err()
        .expect("custom CA without a certificate must reject plugin construction");
    assert_eq!(error, "openid_connect outbound HTTP client build failed");
}

#[test]
fn oidc_construction_rejects_mixed_custom_ca_with_and_without_san() {
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let certificate =
        rcgen::generate_simple_self_signed(vec!["idp.example.com".to_string()]).expect("generate OIDC CA certificate");
    let ca_secret = |name: &str, pem: &[u8]| k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([("ca.crt".to_string(), ByteString(pem.to_vec()))])),
        ..Default::default()
    };

    for with_san in [false, true] {
        let mut config = base_config();
        config.tls.verify = Some(true);
        config.tls.validation = Some(OutboundTlsValidation {
            ca_certificate_refs: Some(
                ["idp-ca-valid", "idp-ca-empty"]
                    .map(|name| CustomCACertificateRef {
                        group: Some(String::new()),
                        kind: "Secret".to_string(),
                        name: name.to_string(),
                        namespace: None,
                    })
                    .to_vec(),
            ),
            subject_alt_names: with_san.then(|| {
                vec![SubjectAltName {
                    san_type: SubjectAltNameType::Hostname,
                    hostname: Some("idp.example.com".to_string()),
                    uri: None,
                }]
            }),
            ..Default::default()
        });
        config.resolved_ca_certificates = Some(vec![
            ca_secret("idp-ca-valid", certificate.cert.pem().as_bytes()),
            ca_secret("idp-ca-empty", b""),
        ]);

        let error = actual_oidc(&config, "default".to_string())
            .err()
            .expect("a mixed valid and empty CA list must reject OIDC construction");
        let expected = if with_san {
            "openid_connect SAN-pinning TLS config build failed"
        } else {
            "openid_connect outbound HTTP client build failed"
        };
        assert_eq!(error, expected);
    }
}

#[test]
fn oidc_construction_fails_closed_when_mtls_secret_is_unresolved() {
    use edgion_resources::resources::common::SecretObjectReference;

    let mut config = base_config();
    config.tls.client_certificate_ref = Some(SecretObjectReference {
        group: None,
        kind: None,
        name: "idp-client".to_string(),
        namespace: None,
    });

    let error = actual_oidc(&config, "default".to_string())
        .err()
        .expect("unresolved mTLS Secret must reject plugin construction");
    assert_eq!(error, "openid_connect outbound HTTP client build failed");
}

#[test]
fn oidc_san_construction_fails_closed_when_custom_ca_is_invalid() {
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let mut config = base_config();
    config.tls.verify = Some(true);
    config.tls.validation = Some(OutboundTlsValidation {
        ca_certificate_refs: Some(vec![CustomCACertificateRef {
            group: Some(String::new()),
            kind: "Secret".to_string(),
            name: "idp-ca".to_string(),
            namespace: None,
        }]),
        subject_alt_names: Some(vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some("idp.example.com".to_string()),
            uri: None,
        }]),
        ..Default::default()
    });
    config.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("idp-ca".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "ca.crt".to_string(),
            ByteString(b"not-a-pem".to_vec()),
        )])),
        ..Default::default()
    }]);

    let error = actual_oidc(&config, "default".to_string())
        .err()
        .expect("invalid SAN-pinning CA must reject plugin construction");
    assert_eq!(error, "openid_connect SAN-pinning TLS config build failed");
}

#[test]
fn oidc_default_client_construction_succeeds_with_egress_resolver() {
    actual_oidc(&base_config(), "default".to_string())
        .expect("default OIDC client must build with dial-time egress policy");
}

#[tokio::test]
async fn oidc_egress_blocks_link_local_literal() {
    let err = super::oidc_egress_resolver()
        .resolve_socket_addrs("169.254.169.254", 443)
        .await
        .expect_err("link-local OIDC targets must fail closed");
    assert!(
        err.contains("egress policy blocked all resolved addresses"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn oidc_egress_dns_failure_fails_closed() {
    use crate::plugins::http::common::outbound::egress::{
        default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
    };
    use edgion_common::dns::ResolverSpec;

    let resolver = EgressResolver::new(
        ResolverSpec::StandardDns {
            servers: vec!["not-a-dns-server".to_string()],
        },
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    );
    let err = resolver
        .resolve_socket_addrs("idp.example.com", 443)
        .await
        .expect_err("OIDC DNS failure must fail closed");
    assert!(
        err.contains("DNS resolution failed for outbound target"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn oidc_egress_allows_loopback_in_unit_tests() {
    let addrs = super::oidc_egress_resolver()
        .resolve_socket_addrs("127.0.0.1", 443)
        .await
        .expect("unit tests allow loopback through EgressResolver");
    assert_eq!(addrs, vec!["127.0.0.1:443".parse().unwrap()]);
}

#[test]
fn oidc_admit_blocks_link_local_ip_literal_url() {
    let err = super::admit_oidc_outbound_url("http://169.254.169.254/latest/meta-data/")
        .expect_err("link-local IP literal must be classified before reqwest dials");
    assert_eq!(err, super::OidcDependencyFailure::DestinationRejected);
}

#[test]
fn oidc_admit_blocks_ipv6_link_local_ip_literal_url() {
    super::admit_oidc_outbound_url("http://[fe80::1]/jwks")
        .expect_err("IPv6 link-local IP literal must be classified before reqwest dials");
}

#[test]
fn oidc_admit_allows_hostname_url() {
    super::admit_oidc_outbound_url("https://idp.example.com/jwks")
        .expect("hostname URLs are classified by EgressResolver at dial time");
}

#[test]
fn oidc_admit_allows_loopback_ip_literal_in_unit_tests() {
    super::admit_oidc_outbound_url("http://127.0.0.1:1/.well-known/openid-configuration")
        .expect("unit tests allow loopback IP literals");
}

#[tokio::test]
async fn oidc_discovery_fetch_rejects_link_local_ip_literal() {
    let mut config = base_config();
    config.discovery = "http://169.254.169.254/.well-known/openid-configuration".to_string();
    let plugin = OpenidConnect::new(&config, "default".to_string());
    let err = plugin
        .get_or_fetch_discovery()
        .await
        .expect_err("link-local discovery URL must fail closed before dial");
    assert_eq!(err.to_string(), "discovery destination denied");
}

// -----------------------------------------------------------------------
// Task 11: JWKS staleness -> dye records.
//
// `get_or_fetch_jwks` (discovery_jwks.rs) is the sole consumer of the
// provider's `get_jwks`, which transparently serves a stale cached set on a
// failed fetch. These tests pin the two outcomes it must record:
// - stale serve (fetch failed, cache still populated) -> the token is still
//   validated against the degraded set, so it's a fail-OPEN record:
//   `jwks-stale-cache`.
// - no cache at all (fetch failed, nothing to fall back to) -> the caller
//   cannot validate the token, so it's a fail-CLOSE record:
//   `jwks-unreachable`.
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_get_or_fetch_jwks_stale_serve_records_fail_open() {
    use super::super::jwks_provider::{ScriptedHttp, JWKS_PATH};

    let script = ScriptedHttp::new();
    let jwks_ok = serde_json::json!({
        "keys": [{"kty":"oct","kid":"k0","k":"AAAAAA","alg":"HS256"}],
    })
    .to_string()
    .into_bytes();
    script.enqueue_http(JWKS_PATH, 200, jwks_ok);
    script.enqueue_http_repeat(JWKS_PATH, 503, Vec::new());

    let mut cfg = base_config();
    cfg.jwks_min_refresh_interval = 0; // force_refresh must not be throttled
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    plugin.jwks_provider.install_scripted_http(script);
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: ScriptedHttp::origin().to_string(),
            jwks_uri: ScriptedHttp::jwks_url(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    // Populate the cache. This is a fresh successful fetch, not a stale
    // serve, so no record_outcome call is expected here (an unexpected call
    // on a MockPluginSession with no expectation installed panics).
    let mut populate_session = MockPluginSession::new();
    plugin
        .get_or_fetch_jwks(&mut populate_session, false)
        .await
        .expect("initial fetch populates cache");

    // force_refresh=true bypasses the TTL fast-path, hits the now-failing
    // endpoint, and falls back to the cache populated above.
    let mut stale_session = MockPluginSession::new();
    stale_session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "jwks-stale-cache")
        .times(1)
        .return_const(());
    let lookup = plugin
        .get_or_fetch_jwks(&mut stale_session, true)
        .await
        .expect("stale fallback must still succeed");
    assert_eq!(
        lookup.keys.len(),
        1,
        "stale cache must still be usable for verification"
    );
}

#[tokio::test]
async fn test_get_or_fetch_jwks_unreachable_no_cache_records_fail_close() {
    use super::super::jwks_provider::{ScriptedHttp, JWKS_PATH};

    let script = ScriptedHttp::new();
    script.enqueue_http_repeat(JWKS_PATH, 503, Vec::new());

    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    plugin.jwks_provider.install_scripted_http(script);
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: ScriptedHttp::origin().to_string(),
            jwks_uri: ScriptedHttp::jwks_url(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: None,
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;

    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "jwks-unreachable")
        .times(1)
        .return_const(());

    let err = plugin
        .get_or_fetch_jwks(&mut session, false)
        .await
        .expect_err("no cache + failed fetch must error");
    assert!(
        err.to_string().contains("503") || err.to_string().contains("status"),
        "expected status error, got: {err}"
    );
}

fn oidc_tls_owner(with_san: bool) -> (edgion_resources::EdgionPlugins, String, String) {
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    let cert = rcgen::generate_simple_self_signed(vec!["idp.example.com".into()]).unwrap();
    let pem = cert.cert.pem();
    let key = cert.signing_key.serialize_pem();
    let mut config = base_config();
    config.tls.verify = Some(true);
    config.tls.validation = Some(OutboundTlsValidation {
        ca_certificate_refs: Some(vec![serde_json::from_value(
            json!({"name":"ca","kind":"Secret","group":"core"}),
        )
        .unwrap()]),
        subject_alt_names: with_san.then(|| {
            vec![SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some("idp.example.com".into()),
                uri: None,
            }]
        }),
        ..Default::default()
    });
    config.tls.client_certificate_ref = Some(serde_json::from_value(json!({"name":"client","group":"core"})).unwrap());
    let secret = |name: &str, fields: &[(&str, &[u8])]| k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            resource_version: Some("7".into()),
            ..Default::default()
        },
        data: Some(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), k8s_openapi::ByteString(v.to_vec())))
                .collect(),
        ),
        ..Default::default()
    };
    config.resolved_ca_certificates = Some(vec![secret("ca", &[("ca.crt", pem.as_bytes())])]);
    config.resolved_client_certificate = Some(secret(
        "client",
        &[("tls.crt", pem.as_bytes()), ("tls.key", key.as_bytes())],
    ));
    let owner = serde_json::from_value(json!({"metadata":{"name":"oidc","uid":"first"},"spec":{
        "requestPlugins":[{"type":"OpenidConnect","config":config}]}}))
    .unwrap();
    (owner, pem, key)
}

#[test]
fn oidc_actual_tls_identity_precedes_both_crypto_branches() {
    use edgion_common::link_sys_core::outbound_tls::{resolve_validated_outbound_tls, EffectiveCa};
    use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
    use edgion_resources::EdgionPlugin;
    let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
    for with_san in [false, true] {
        let (owner, pem, key) = oidc_tls_owner(with_san);
        for case in [
            "matching",
            "configmap",
            "ca-name",
            "ca-no-name",
            "ca-ns",
            "ca-no-ns",
            "ca-empty-ns",
            "client-name",
            "client-no-name",
            "client-ns",
            "client-no-ns",
            "marker",
            "client-marker",
            "retarget",
            "no-ref",
            "no-ca",
            "no-client",
            "owner",
            "no-owner",
            "owner-ns",
            "claim-priority",
        ] {
            let mut candidate = owner.clone();
            let EdgionPlugin::OpenidConnect(config) = &mut candidate.spec.request_plugins.as_mut().unwrap()[0].plugin
            else {
                panic!()
            };
            match case {
                "configmap" => {
                    config
                        .tls
                        .validation
                        .as_mut()
                        .unwrap()
                        .ca_certificate_refs
                        .as_mut()
                        .unwrap()[0]
                        .kind = "ConfigMap".into();
                    config.resolved_ca_certificates.as_mut().unwrap()[0]
                        .metadata
                        .annotations = Some(std::collections::BTreeMap::from([(
                        edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE.into(),
                        edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE_CONFIG_MAP.into(),
                    )]));
                }
                "ca-name" => config.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = Some("other".into()),
                "ca-no-name" => config.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = None,
                "ca-ns" => {
                    config.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("other".into())
                }
                "ca-no-ns" => config.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = None,
                "ca-empty-ns" => {
                    config.resolved_ca_certificates.as_mut().unwrap()[0].metadata.namespace = Some("".into())
                }
                "client-name" => {
                    config.resolved_client_certificate.as_mut().unwrap().metadata.name = Some("other".into())
                }
                "client-no-name" => config.resolved_client_certificate.as_mut().unwrap().metadata.name = None,
                "client-ns" => {
                    config.resolved_client_certificate.as_mut().unwrap().metadata.namespace = Some("other".into())
                }
                "client-no-ns" => config.resolved_client_certificate.as_mut().unwrap().metadata.namespace = None,
                "marker" | "client-marker" => {
                    let secret = if case == "marker" {
                        &mut config.resolved_ca_certificates.as_mut().unwrap()[0]
                    } else {
                        config.resolved_client_certificate.as_mut().unwrap()
                    };
                    secret.metadata.annotations = Some(std::collections::BTreeMap::from([(
                        edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE.into(),
                        "ConfigMap".into(),
                    )]));
                }
                "retarget" => {
                    config
                        .tls
                        .validation
                        .as_mut()
                        .unwrap()
                        .ca_certificate_refs
                        .as_mut()
                        .unwrap()[0]
                        .name = "other".into()
                }
                "no-ref" => config.tls.validation.as_mut().unwrap().ca_certificate_refs = None,
                "no-ca" => config.resolved_ca_certificates = None,
                "no-client" => config.resolved_client_certificate = None,
                "claim-priority" => {
                    config.claims_to_headers = Some(HashMap::from([("sub".into(), "bad header".into())]))
                }
                _ => {}
            }
            match case {
                "owner" | "claim-priority" => candidate.metadata.name = Some("bad/name".into()),
                "no-owner" => candidate.metadata.name = None,
                "owner-ns" => candidate.metadata.namespace = Some("Bad_NS".into()),
                _ => {}
            }
            let prepared = PreparedPluginReferences::from_plugins(&candidate);
            let binding = prepared.bind(&candidate).unwrap();
            let entry = binding.request_entries().next().unwrap();
            let proof = OpenidConnect::validate_tls_entry(&entry, &defaults);
            if matches!(case, "matching" | "configmap") {
                let resolved = resolve_validated_outbound_tls(&proof.unwrap(), true, &defaults).unwrap();
                let EffectiveCa::Custom(bundles) = resolved.ca else {
                    panic!("actual custom CA must survive")
                };
                assert_eq!(bundles, vec![pem.as_bytes().to_vec()]);
                let (cert, private) = resolved.client_identity.unwrap();
                assert_eq!(cert, pem.as_bytes());
                assert_eq!(private.as_slice(), key.as_bytes());
                let plugin = OpenidConnect::try_new(&entry, &defaults).unwrap();
                assert!(plugin.config.resolved_ca_certificates.is_none());
                assert!(plugin.config.resolved_client_certificate.is_none());
            } else {
                assert!(proof.is_err(), "{case}");
                let error = OpenidConnect::try_new(&entry, &defaults)
                    .err()
                    .expect("actual identity must reject before parsing");
                if case == "claim-priority" {
                    assert_eq!(error, "claim-to-header destination must be a valid HTTP header name");
                } else {
                    assert_eq!(
                        error,
                        if with_san {
                            "openid_connect outbound TLS resolution failed"
                        } else {
                            "openid_connect outbound HTTP client build failed"
                        },
                        "{case}"
                    );
                }
            }
        }
    }
}

#[test]
fn oidc_both_tls_branches_use_the_captured_global_generation() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::openid_connect::plugin::tests",
        "oidc_both_tls_branches_use_the_captured_global_generation",
        "EDGION_OIDC_CAPTURED_TLS_TEST",
        "captured",
        || {
            use edgion_common::link_sys_core::outbound_tls::{
                outbound_global_default, resolve_validated_outbound_tls, set_outbound_global_default, EffectiveCa,
            };
            use edgion_resources::resources::common::OutboundTlsConfig;
            use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
            use edgion_resources::EdgionPlugin;
            let previous = outbound_global_default();
            for with_san in [false, true] {
                let (mut owner, pem, key) = oidc_tls_owner(with_san);
                let EdgionPlugin::OpenidConnect(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin
                else {
                    panic!()
                };
                let mut global = OutboundTlsConfig {
                    verify: true,
                    client_certificate_ref: config.tls.client_certificate_ref.take(),
                    resolved_client_certificate: config.resolved_client_certificate.take(),
                    ..Default::default()
                };
                // Local SAN replaces the CA axis; mTLS still inherits in both branches.
                global.client_certificate_ref.as_mut().unwrap().namespace = Some("default".into());
                if !with_san {
                    global.validation = config.tls.validation.take();
                    for reference in global
                        .validation
                        .as_mut()
                        .unwrap()
                        .ca_certificate_refs
                        .as_mut()
                        .unwrap()
                    {
                        reference.namespace = Some("default".into());
                    }
                    global.resolved_ca_certificates = config.resolved_ca_certificates.take();
                }
                let captured = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(global.clone());
                global.resolved_client_certificate.as_mut().unwrap().metadata.name = Some("wrong".into());
                let invalid = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(global);
                set_outbound_global_default(invalid.clone());
                let prepared = PreparedPluginReferences::from_plugins(&owner);
                let binding = prepared.bind(&owner).unwrap();
                let entry = binding.request_entries().next().unwrap();
                let proof = OpenidConnect::validate_tls_entry(&entry, &captured).unwrap();
                let resolved = resolve_validated_outbound_tls(&proof, true, &captured).unwrap();
                let EffectiveCa::Custom(bundles) = resolved.ca else {
                    panic!("captured/local CA must survive")
                };
                assert_eq!(bundles, vec![pem.as_bytes().to_vec()]);
                let (cert, private) = resolved.client_identity.unwrap();
                assert_eq!(cert, pem.as_bytes());
                assert_eq!(private.as_slice(), key.as_bytes());
                assert!(OpenidConnect::try_new(&entry, &captured).is_ok());
                set_outbound_global_default(captured.clone());
                assert!(OpenidConnect::try_new(&entry, &invalid).is_err());
            }
            set_outbound_global_default(previous);
        },
    );
}

fn signing_policy_script() -> Arc<super::super::jwks_provider::ScriptedHttp> {
    use super::super::jwks_provider::{ScriptedHttp, DISCOVERY_PATH, JWKS_PATH};
    use base64::Engine;

    let script = ScriptedHttp::new();
    script.enqueue_http_repeat(
        DISCOVERY_PATH,
        200,
        json!({
            "issuer": ScriptedHttp::origin(), "jwks_uri": ScriptedHttp::jwks_url(),
            "authorization_endpoint": "http://oidc.test/authorize",
            "token_endpoint": "http://oidc.test/token",
            "userinfo_endpoint": "http://oidc.test/userinfo"
        })
        .to_string(),
    );
    script.enqueue_http_repeat(
        JWKS_PATH,
        200,
        json!({"keys": [{
            "kty": "oct", "alg": "HS256", "kid": "policy-key",
            "k": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"signing-policy-test-secret")
        }]})
        .to_string(),
    );
    script
}

fn signing_policy_token(audience: &str, nonce: &str) -> String {
    use super::super::jwks_provider::ScriptedHttp;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("policy-key".to_string());
    jsonwebtoken::encode(
        &header,
        &json!({
            "iss": ScriptedHttp::origin(), "sub": "policy-user", "aud": audience,
            "nonce": nonce, "exp": OpenidConnect::now_unix_secs() + 3600
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"signing-policy-test-secret"),
    )
    .unwrap()
}

#[tokio::test]
async fn prepared_signing_policy_preserves_header_first_and_cold_provider_rejection() {
    use super::super::error::{AuthFailure, OidcVerifyError};
    use super::super::jwks_provider::{DISCOVERY_PATH, JWKS_PATH};

    let token = signing_policy_token("client", "expected-nonce");
    for expected in [Some("HS256"), None] {
        let mut config = base_config();
        config.token_signing_alg = expected.map(str::to_owned);
        assert!(config.get_validation_error().is_none());
        let plugin = actual_oidc(&config, "default".to_string()).expect("policy preserves construction");
        let script = signing_policy_script();
        plugin.jwks_provider.install_scripted_http(script.clone());
        // Both provider caches remain cold. Even successful routes must not be consulted.
        let mut session = MockPluginSession::new();
        assert_eq!(
            plugin
                .verify_jwt_token_internal(&mut session, "bad.header.payload", true, None, true)
                .await,
            Err(OidcVerifyError::InvalidFormat)
        );
        assert_eq!(
            plugin
                .verify_jwt_token_internal(&mut session, &token, true, None, true)
                .await,
            Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm))
        );
        assert_eq!(script.hits(DISCOVERY_PATH), 0);
        assert_eq!(script.hits(JWKS_PATH), 0);
    }
}

#[tokio::test]
async fn prepared_signing_policy_retains_old_generation_and_id_token_audience_nonce() {
    use super::super::error::{AuthFailure, OidcVerifyError};
    use super::super::jwks_provider::{DISCOVERY_PATH, JWKS_PATH};

    let mut config = base_config();
    config.token_signing_alg = Some("HS256".to_string());
    config.allowed_signing_algs = Some(vec!["HS256".to_string()]);
    config.audiences = Some(vec!["resource-api".to_string()]);
    let old = Arc::new(OpenidConnect::new(&config, "default".to_string()));
    let old_script = signing_policy_script();
    old.jwks_provider.install_scripted_http(old_script.clone());
    let token = signing_policy_token("resource-api", "expected-nonce");
    let mut session = MockPluginSession::new();
    assert!(old.verify_jwt_token(&mut session, &token).await.is_ok());

    config.token_signing_alg = Some("RS256".to_string());
    config.allowed_signing_algs = Some(vec!["RS256".to_string()]);
    let new = OpenidConnect::new(&config, "default".to_string());
    let new_script = signing_policy_script();
    new.jwks_provider.install_scripted_http(new_script.clone());
    assert_eq!(
        new.verify_jwt_token(&mut session, &token).await,
        Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm))
    );
    assert!(old.verify_jwt_token(&mut session, &token).await.is_ok());
    assert_eq!(new_script.hits(DISCOVERY_PATH), 0);
    assert_eq!(new_script.hits(JWKS_PATH), 0);
    assert_eq!(old_script.hits(DISCOVERY_PATH), 1);
    assert_eq!(old_script.hits(JWKS_PATH), 1);

    let id_token = signing_policy_token("client", "expected-nonce");
    assert!(old
        .validate_id_token_nonce(&mut session, &id_token, "expected-nonce")
        .await
        .is_ok());
    assert_eq!(
        old.validate_id_token_nonce(&mut session, &id_token, "different-nonce")
            .await,
        Err(OidcVerifyError::Auth(AuthFailure::InvalidNonce))
    );
    assert!(old
        .validate_id_token_nonce(&mut session, &token, "expected-nonce")
        .await
        .is_err());
    assert!(old.verify_jwt_token(&mut session, &id_token).await.is_err());
}

#[tokio::test]
async fn introspection_endpoint_presence_precedes_cache_and_url_admission_follows_it() {
    const DENIED: &str = "http://169.254.169.254/introspect";
    for (configured, discovered, available) in [
        (None, None, false),
        (Some(""), Some(""), false),
        (Some(""), None, false),
        (None, Some(""), false),
        (Some(DENIED), None, true),
        (None, Some(DENIED), true),
        (Some(""), Some(DENIED), true),
        (Some(DENIED), Some(""), true),
        (Some(DENIED), Some("https://other.example/introspect"), true),
    ] {
        let mut cfg = base_config();
        cfg.introspection_cache_ttl = 60;
        let mut plugin = OpenidConnect::new(&cfg, "default".to_owned());
        // Exercise runtime selection independently of resource admission.
        plugin.config.introspection_endpoint = configured.map(str::to_owned);
        plugin
            .jwks_provider
            .seed_discovery(DiscoveryDocument {
                issuer: "https://issuer.example.com".to_owned(),
                jwks_uri: "https://issuer.example.com/jwks".to_owned(),
                authorization_endpoint: None,
                token_endpoint: None,
                introspection_endpoint: discovered.map(str::to_owned),
                userinfo_endpoint: None,
                end_session_endpoint: None,
                revocation_endpoint: None,
            })
            .await;
        let claims = json!({
            "active": true, "iss": "https://issuer.example.com",
            "exp": OpenidConnect::now_unix_secs() + 120
        });
        plugin.cache_introspection_claims("cached-token", &claims).await;
        let mut session = MockPluginSession::new();
        let result = plugin
            .verify_token_via_introspection(&mut session, "cached-token")
            .await;
        if available {
            assert_eq!(result.unwrap(), claims);
        } else {
            assert_eq!(result.unwrap_err().log_tag(), "introspect endpoint missing");
        }
    }
}

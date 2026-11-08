use super::super::jwt_common::{select_jwk, JwkSelectError};
use super::{DiscoveryDocument, IntrospectionCacheEntry, OidcSessionCookie, OpenidConnect};
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::plugins::runtime::DyeOutcome;
use crate::plugins::runtime::PluginLog;
use crate::plugins::runtime::RequestFilter;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
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

#[test]
fn exhausted_fixed_header_budget_still_clears_all_claim_destinations() {
    let mut cfg = base_config();
    cfg.set_access_token_header = true;
    cfg.max_header_value_bytes = 15;
    cfg.max_total_header_bytes = 15;
    cfg.claims_to_headers = Some(HashMap::from([("sub".into(), "X-Sub".into())]));
    let plugin = OpenidConnect::new(&cfg, "default".into());
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header_value()
        .withf(|name, _| name == "X-Access-Token")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "x-sub")
        .times(1)
        .returning(|_| Ok(()));
    plugin
        .apply_upstream_headers(&mut session, "a", None, None, &json!({"sub":"trusted"}))
        .unwrap();
}

#[tokio::test]
async fn unauthenticated_pass_clears_claim_destinations_and_fails_closed_on_mutation_error() {
    for fail in [false, true] {
        let mut cfg = base_config();
        cfg.bearer_only = false;
        cfg.unauth_action = UnauthAction::Pass;
        cfg.claims_to_headers = Some(HashMap::from([("sub".into(), "X-Sub".into())]));
        let plugin = OpenidConnect::new(&cfg, "default".into());
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/protected".to_string());
        install_request_cookies(&mut session, "");
        session
            .expect_remove_request_header()
            .withf(|name| name == "x-sub")
            .times(1)
            .returning(move |_| {
                if fail {
                    Err(std::io::Error::other("private error").into())
                } else {
                    Ok(())
                }
            });
        let mut log = PluginLog::new("OpenidConnect");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            if fail {
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                }
            } else {
                PluginRunningResult::GoodNext
            }
        );
        assert!(!log.contains("private"));
    }
}

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
    // Keep the frozen ignored local-IdP fixtures on their existing test helper.
    pub(super) fn looks_like_jwt(token: &str) -> bool {
        Self::decode_jwt_header(token).is_some()
    }

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
        "access_token": "private-access", "created_at": 1, "session_expires_at": 3601,
        "id_token": "private-id", "refresh_token": "private-refresh"
    }))
    .unwrap();
    let response: super::TokenEndpointResponse = serde_json::from_value(json!({
        "access_token": "private-access", "id_token": "private-id", "refresh_token": "private-refresh"
    }))
    .unwrap();
    let introspection = IntrospectionCacheEntry {
        claims: Arc::new(json!({"sub": "private-identity"})),
    };
    for value in [&state as &dyn std::fmt::Debug, &session, &response, &introspection] {
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

pub(super) fn base_config() -> OpenidConnectConfig {
    OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        ..Default::default()
    }
}

pub(super) fn plugin_log_entries(plugin_log: &PluginLog) -> Vec<String> {
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

#[test]
fn claim_sanitation_precedes_fixed_outputs_even_when_they_exhaust_the_budget() {
    for fail in [false, true] {
        let mut cfg = base_config();
        cfg.claims_to_headers = Some(HashMap::from([("role".into(), "X-Role".into())]));
        cfg.set_access_token_header = true;
        cfg.max_total_header_bytes = 15;
        let plugin = OpenidConnect::new(&cfg, "default".into());
        let mut session = MockPluginSession::new();
        let mut sequence = mockall::Sequence::new();
        session
            .expect_remove_request_header()
            .withf(|name| name == "x-role")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |_| {
                if fail {
                    Err("injected removal failure".into())
                } else {
                    Ok(())
                }
            });
        session
            .expect_set_request_header_value()
            .withf(|name, value| name == "X-Access-Token" && value == "a")
            .times(if fail { 0 } else { 1 })
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        let result = plugin.apply_upstream_headers(&mut session, "a", None, None, &json!({"role":"user"}));
        assert_eq!(result.is_err(), fail);
    }
}

#[tokio::test]
async fn authenticated_bearer_claim_cleanup_failure_prevents_upstream_continuation() {
    let mut cfg = base_config();
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.introspection_cache_ttl = 60;
    cfg.claims_to_headers = Some(HashMap::from([("role".into(), "X-Role".into())]));
    let plugin = OpenidConnect::new(&cfg, "default".into());
    plugin
        .jwks_provider
        .seed_discovery(DiscoveryDocument {
            issuer: "https://idp.example.com".into(),
            jwks_uri: "https://idp.example.com/jwks".into(),
            authorization_endpoint: None,
            token_endpoint: None,
            introspection_endpoint: Some("https://idp.example.com/introspect".into()),
            userinfo_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
        })
        .await;
    plugin
        .cache_introspection_claims(
            "cached-token",
            &Arc::new(json!({
                "active":true, "iss":"https://idp.example.com", "exp":OpenidConnect::now_unix_secs()+3600
            })),
        )
        .await;
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/".to_owned());
    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::AUTHORIZATION, "Bearer cached-token".parse().unwrap());
    session.expect_request_header_map().return_const(headers);
    session
        .expect_remove_request_header()
        .withf(|name| name == "x-role")
        .times(1)
        .returning(|_| Err("injected removal failure".into()));
    session.expect_set_request_header_value().never();
    session.expect_remove_request_cookies().never();
    assert_eq!(
        plugin
            .run_request(&mut session, &mut PluginLog::new("OpenidConnect"))
            .await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

/// Logout Set-Cookie lines for the default cookie configuration: the single session
/// cookie, the chunk count, all 16 chunk slots, then the state cookie.
pub(super) fn expected_logout_set_cookies() -> Vec<String> {
    expected_logout_set_cookies_with_state_path("/.edgion/oidc/callback")
}

pub(super) fn expected_logout_set_cookies_with_state_path(state_path: &str) -> Vec<String> {
    let mut cookies = vec![
        expire_line("edgion_oidc_session"),
        expire_line("edgion_oidc_session_chunks"),
    ];
    cookies.extend((0..16).map(|index| expire_line(&format!("edgion_oidc_session_{index}"))));
    cookies.push(format!(
        "edgion_oidc_state=; Path={state_path}; SameSite=Lax; Max-Age=0; HttpOnly; Secure"
    ));
    cookies
}

/// Every Gateway-owned cookie name stripped before proxying, in strip order.
pub(super) fn gateway_owned_cookie_names() -> Vec<String> {
    let mut names = vec![
        "edgion_oidc_session".to_string(),
        "edgion_oidc_state".to_string(),
        "edgion_oidc_session_chunks".to_string(),
    ];
    names.extend((0..16).map(|index| format!("edgion_oidc_session_{index}")));
    names
}

/// Shared Cookie header for `get_cookie` and `strip_cookie` on a mock session.
pub(super) fn install_request_cookies(
    session: &mut MockPluginSession,
    cookie_header: &str,
) -> Arc<Mutex<Option<String>>> {
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
    let for_edit = Arc::clone(&cookie);
    session.expect_remove_request_cookies().returning(move |names| {
        use crate::request_cookie::{CookieEditOutcome, CookieEditPlan};
        let mut stored = for_edit.lock();
        let mut headers = http::HeaderMap::new();
        if let Some(value) = stored.as_ref() {
            headers.insert(http::header::COOKIE, http::HeaderValue::from_str(value).unwrap());
        }
        match crate::request_cookie::plan_removal(&headers, names)? {
            CookieEditPlan::Absent => Ok(CookieEditOutcome::Absent),
            CookieEditPlan::NoChange => Ok(CookieEditOutcome::NoChange),
            CookieEditPlan::Remove => {
                *stored = None;
                Ok(CookieEditOutcome::Removed)
            }
            CookieEditPlan::Replace(value) => {
                assert!(value.is_sensitive());
                *stored = Some(std::str::from_utf8(value.as_bytes()).unwrap().to_owned());
                Ok(CookieEditOutcome::Replaced)
            }
        }
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
fn logout_redirect_target_borrows_configured_text_and_preserves_prefix_rules() {
    let mut plugin = OpenidConnect::new(&base_config(), "default".to_string());
    for (configured, expected) in [
        (None, "/"),
        (Some(""), "/"),
        (Some("/after"), "/after"),
        (Some("http://app.example/after"), "http://app.example/after"),
        (Some("https://app.example/after"), "https://app.example/after"),
        (Some("HTTPS://app.example/after"), "/"),
        (Some("relative"), "/"),
        (Some("//app.example/after"), "//app.example/after"),
        (Some(r"/\app.example/after"), r"/\app.example/after"),
    ] {
        // Exercise the helper's defensive branches without changing admission.
        plugin.config.post_logout_redirect_uri = configured.map(str::to_owned);
        let target = plugin.logout_redirect_target();
        assert_eq!(target, expected);
        if configured == Some(expected) {
            assert_eq!(
                target.as_ptr(),
                plugin.config.post_logout_redirect_uri.as_ref().unwrap().as_ptr()
            );
        }
    }
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
        claims: Arc::new(serde_json::json!({"sub": "test"})),
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
    session
        .expect_remove_request_cookies()
        .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Absent));
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
    session
        .expect_remove_request_cookies()
        .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Absent));
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
    session.expect_request_query().return_const(Some("x=1"));
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

#[test]
fn authorization_redirect_preserves_random_lengths_and_pkce_binding() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let mut cfg = base_config();
    cfg.use_nonce = true;
    cfg.use_pkce = true;
    cfg.redirect_uri = Some("https://app.example/callback".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let discovery = DiscoveryDocument {
        issuer: "https://idp.example.com".to_string(),
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        authorization_endpoint: Some("https://idp.example.com/authorize".to_string()),
        token_endpoint: None,
        introspection_endpoint: None,
        userinfo_endpoint: None,
        end_session_endpoint: None,
        revocation_endpoint: None,
    };
    for (raw_query, original_url) in [
        (None, "/orders"),
        (Some(""), "/orders"),
        (Some("x=1"), "/orders?x=1"),
        (Some("x=1&x=2&encoded=a%20b%26c"), "/orders?x=1&x=2&encoded=a%20b%26c"),
    ] {
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/orders".to_string());
        session.expect_get_query().never();
        session.expect_request_query().return_const(raw_query);
        let (redirect, cookie) = plugin.build_authorization_redirect(&session, &discovery).unwrap();
        let url = url::Url::parse(&redirect).unwrap();
        let query: HashMap<_, _> = url.query_pairs().collect();
        let encoded = cookie.split_once('=').unwrap().1.split(';').next().unwrap();
        let payload: super::AuthorizationStateCookie =
            OpenidConnect::decode_signed_cookie_payload(encoded, plugin.session_cipher().unwrap()).unwrap();
        assert_eq!(payload.original_url, original_url);
        assert_eq!(query["state"], payload.state);
        assert_eq!(query["nonce"], payload.nonce.as_deref().unwrap());
        let verifier = payload.code_verifier.as_deref().unwrap();
        for (value, bytes, chars) in [
            (payload.state.as_str(), 24, 32),
            (payload.nonce.as_deref().unwrap(), 24, 32),
            (verifier, 64, 86),
        ] {
            assert_eq!(value.len(), chars);
            assert!(value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
            assert_eq!(URL_SAFE_NO_PAD.decode(value).unwrap().len(), bytes);
        }
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        );
    }
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
    session
        .expect_remove_request_cookies()
        .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Absent));
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
    session.expect_request_query().return_const(Some("x=1"));
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
    session
        .expect_remove_request_cookies()
        .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Absent));
    setup_header_mocks(&mut session, &[("host", "api.example.com")]);
    session.expect_downstream_is_tls().returning(|| Some(true));
    session.expect_request_query().return_const(None);
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
    session
        .expect_remove_request_cookies()
        .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Absent));
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
    session
        .expect_remove_request_cookies()
        .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Absent));
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
        access_token: "access-token".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
        expires_at: Some(now + 60),
        id_token: Some("id-token-hint".to_string()),
        refresh_token: Some("refresh-token".to_string()),
    };
    let encoded_cookie = OpenidConnect::encode_signed_cookie_payload(
        &session_cookie_payload,
        plugin.session_cipher().expect("prepared session cipher"),
    )
    .expect("encode session cookie");

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");

    session.expect_get_path().return_const("/auth/logout".to_string());
    let cookie_header = format!("edgion_oidc_session={encoded_cookie}");
    setup_header_mocks(
        &mut session,
        &[("host", "api.example.com"), ("cookie", cookie_header.as_str())],
    );
    session.expect_downstream_is_tls().returning(|| Some(true));
    session.expect_write_response_header().returning(move |resp, end| {
        assert!(end);
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
        assert_eq!(cookies, expected_logout_set_cookies());
        Ok(())
    });

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
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
    session
        .expect_remove_request_cookies()
        .returning(|_| Ok(crate::request_cookie::CookieEditOutcome::Absent));
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

fn original_cookie_frame(nonce: &[u8; 12], encrypted: &[u8]) -> String {
    let nonce_b64 = URL_SAFE_NO_PAD.encode(nonce);
    let encrypted_b64 = URL_SAFE_NO_PAD.encode(encrypted);
    format!("{}.{}", nonce_b64, encrypted_b64)
}

fn presized_string_cookie_frame(nonce: &[u8; 12], encrypted: &[u8]) -> Result<String, ()> {
    let nonce_len = base64::encoded_len(nonce.len(), false).ok_or(())?;
    let encrypted_len = base64::encoded_len(encrypted.len(), false).ok_or(())?;
    let len = nonce_len
        .checked_add(1)
        .and_then(|len| len.checked_add(encrypted_len))
        .ok_or(())?;
    let mut output = String::with_capacity(len);
    URL_SAFE_NO_PAD.encode_string(nonce, &mut output);
    output.push('.');
    URL_SAFE_NO_PAD.encode_string(encrypted, &mut output);
    Ok(output)
}

#[test]
fn cookie_frame_preserves_complete_bytes_and_capacity() {
    let nonce = [0, 1, 2, 127, 128, 253, 254, 255, 17, 33, 65, 129];
    for len in [
        0, 1, 2, 3, 16, 64, 256, 767, 768, 769, 1535, 1536, 1537, 2800, 3000, 3800,
    ] {
        let encrypted: Vec<u8> = (0..len).map(|index| (index % 256) as u8).collect();
        let frame = OpenidConnect::encode_cookie_frame(&nonce, &encrypted).unwrap();
        assert_eq!(frame, original_cookie_frame(&nonce, &encrypted));
        assert_eq!(frame, presized_string_cookie_frame(&nonce, &encrypted).unwrap());
        assert_eq!(frame.capacity(), frame.len());
        assert_eq!(frame.bytes().filter(|byte| *byte == b'.').count(), 1);
        assert!(!frame.contains('='));
        let (encoded_nonce, encoded_ciphertext) = frame.split_once('.').unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(encoded_nonce).unwrap(), nonce);
        assert_eq!(URL_SAFE_NO_PAD.decode(encoded_ciphertext).unwrap(), encrypted);
    }
}

#[test]
fn cookie_frame_allocation_comparison() {
    let nonce = [17; 12];
    for len in [0, 16, 256, 767, 768, 769, 1535, 1536, 1537, 2800, 3000, 3800] {
        let encrypted = vec![129; len];
        let (original, original_stats) =
            crate::test_allocation_counter::count_allocation_stats(|| original_cookie_frame(&nonce, &encrypted));
        let (string, string_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            presized_string_cookie_frame(&nonce, &encrypted).unwrap()
        });
        let (slice, slice_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            OpenidConnect::encode_cookie_frame(&nonce, &encrypted).unwrap()
        });
        assert_eq!(slice, original);
        assert_eq!(slice, string);
        // Only synthetic input lengths and allocator statistics are emitted.
        println!(
            "cookie-frame input={len}: original calls={} requested={} capacity={}; string calls={} requested={} capacity={}; slice calls={} requested={} capacity={}",
            original_stats.calls, original_stats.requested_bytes, original.capacity(),
            string_stats.calls, string_stats.requested_bytes, string.capacity(),
            slice_stats.calls, slice_stats.requested_bytes, slice.capacity(),
        );
    }
}

#[test]
fn cookie_encoder_preserves_serialization_failure() {
    struct Unserializable;
    impl serde::Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("synthetic serialization failure"))
        }
    }
    let mut config = base_config();
    config.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".into());
    let plugin = OpenidConnect::new(&config, "default".into());
    assert_eq!(
        OpenidConnect::encode_signed_cookie_payload(
            &Unserializable,
            plugin.session_cipher().expect("prepared session cipher"),
        ),
        Err(())
    );
}

#[test]
fn cookie_nonce_decoding_preserves_length_and_error_boundaries() {
    use super::{AuthFailure, OidcVerifyError};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let mut cfg = base_config();
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let cipher = plugin.session_cipher().unwrap();
    let decode = |cookie: &str| OpenidConnect::decode_signed_cookie_payload::<serde_json::Value>(cookie, cipher);
    let malformed = OidcVerifyError::Auth(AuthFailure::MalformedCookie);
    let invalid_signature = OidcVerifyError::Auth(AuthFailure::InvalidCookieSignature);
    for len in [0, 1, 11, 12, 13, 64] {
        let nonce = URL_SAFE_NO_PAD.encode(vec![0u8; len]);
        assert_eq!(
            decode(&format!("{nonce}.")),
            Err(if len == 12 { invalid_signature } else { malformed }),
            "nonce length {len}"
        );
    }
    for nonce in [
        "A",
        "AA=",
        "AB",
        "AAAAAAAAAAAAAAAA=",
        "AAAAAAAAAAAAAAA%",
        "AAAAAAAAAAAAAAA\n",
    ] {
        assert_eq!(decode(&format!("{nonce}.")), Err(malformed));
    }
    assert_eq!(decode("AAAAAAAAAAAAAAAA.%"), Err(malformed));

    let original = OpenidConnect::encode_signed_cookie_payload(&json!({"state": "original"}), cipher).unwrap();
    let (nonce, ciphertext) = original.split_once('.').unwrap();
    let mut changed_nonce = URL_SAFE_NO_PAD.decode(nonce).unwrap();
    changed_nonce[0] ^= 1;
    let tampered = format!("{}.{}", URL_SAFE_NO_PAD.encode(changed_nonce), ciphertext);
    assert_eq!(decode(&tampered), Err(invalid_signature));
}

// Independent persistence oracle for the stateless session schema (gw-plugin-03): the
// access token is persisted, `session_ref` no longer exists, and an empty token is omitted.
#[derive(serde::Serialize)]
struct ExpectedPersistedCookie {
    #[serde(skip_serializing_if = "String::is_empty")]
    access_token: String,
    created_at: u64,
    session_expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

#[test]
fn session_cookie_serializes_exact_json_with_access_token() {
    for token in ["access-token", "", " \t"] {
        for expires_at in [None, Some(42)] {
            for id_token in [None, Some(""), Some("id-\"\\é")] {
                for refresh_token in [None, Some(""), Some("refresh-token")] {
                    let source = OidcSessionCookie {
                        access_token: token.to_owned(),
                        created_at: 7,
                        session_expires_at: 7 + 3600,
                        expires_at,
                        id_token: id_token.map(str::to_owned),
                        refresh_token: refresh_token.map(str::to_owned),
                    };
                    let expected = serde_json::to_vec(&ExpectedPersistedCookie {
                        access_token: token.to_owned(),
                        created_at: 7,
                        session_expires_at: 3607,
                        expires_at,
                        id_token: id_token.map(str::to_owned),
                        refresh_token: refresh_token.map(str::to_owned),
                    })
                    .unwrap();
                    assert_eq!(serde_json::to_vec(&source).unwrap(), expected);
                    let decoded: OidcSessionCookie = serde_json::from_slice(&expected).unwrap();
                    assert_eq!(serde_json::to_vec(&decoded).unwrap(), expected);
                    assert_eq!(decoded.access_token, token);
                    assert_eq!(format!("{source:?}"), "OidcSessionCookie { .. }");
                }
            }
        }
    }
    assert_eq!(
        serde_json::to_string(&OidcSessionCookie {
            access_token: "token".into(),
            created_at: 7,
            session_expires_at: 3607,
            expires_at: Some(42),
            id_token: None,
            refresh_token: Some("refresh".into()),
        })
        .unwrap(),
        r#"{"access_token":"token","created_at":7,"session_expires_at":3607,"expires_at":42,"refresh_token":"refresh"}"#
    );
    let minimal: OidcSessionCookie = serde_json::from_str(r#"{"created_at":7,"session_expires_at":3607}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&minimal).unwrap(),
        r#"{"created_at":7,"session_expires_at":3607}"#
    );
    // Legacy cookies written with the removed `session_ref` still decode; the field is dropped.
    let legacy: OidcSessionCookie = serde_json::from_str(
        r#"{"session_ref":"ref","access_token":"legacy","created_at":7,"session_expires_at":3607,"expires_at":42}"#,
    )
    .unwrap();
    assert_eq!(legacy.access_token, "legacy");
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"access_token":"legacy","created_at":7,"session_expires_at":3607,"expires_at":42}"#
    );
}

fn stateless_session_config() -> OpenidConnectConfig {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Deny;
    cfg.resolved_oidc_client_secret = Some("dummy-secret".to_string());
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    cfg
}

fn capture_appended_set_cookies(session: &mut MockPluginSession) -> Arc<Mutex<Vec<String>>> {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let output = captured.clone();
    session.expect_append_response_header().returning(move |name, value| {
        assert_eq!(name, "Set-Cookie");
        output.lock().push(value.to_string());
        Ok(())
    });
    captured
}

fn set_cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('='))
        .and_then(|rest| rest.split(';').next())
}

#[tokio::test]
async fn rewritten_session_cookie_persists_the_access_token() {
    let plugin = OpenidConnect::new(&stateless_session_config(), "default".to_string());
    let now = OpenidConnect::now_unix_secs();
    let payload = OidcSessionCookie {
        access_token: "access-token".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
        expires_at: Some(now + 60),
        id_token: Some("id-token".to_string()),
        refresh_token: Some("refresh-token".to_string()),
    };
    let source = serde_json::to_value(&payload).unwrap();
    let mut session = MockPluginSession::new();
    let captured = capture_appended_set_cookies(&mut session);
    let mut log = PluginLog::new("OpenidConnect");
    plugin
        .maybe_set_session_cookie_header(&mut session, &payload, 0, &mut log)
        .await;
    assert!(plugin_log_entries(&log).is_empty());

    let cookies = captured.lock().clone();
    assert_eq!(cookies.len(), 2, "single cookie plus the chunk-count expiration");
    assert!(cookies[1].starts_with("edgion_oidc_session_chunks=; "));
    assert!(cookies[1].contains("Max-Age=0"));
    let encoded = set_cookie_value(&cookies[0], "edgion_oidc_session").expect("session cookie");
    let decoded: OidcSessionCookie =
        OpenidConnect::decode_signed_cookie_payload(encoded, plugin.session_cipher().unwrap()).unwrap();
    assert_eq!(decoded.access_token, "access-token");
    assert_eq!(serde_json::to_value(&decoded).unwrap(), source);
}

#[tokio::test]
async fn rewrite_overflow_ends_the_session_with_a_static_tag() {
    let mut cfg = stateless_session_config();
    cfg.max_session_cookie_bytes = 512;
    cfg.max_session_cookie_chunks = 2;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let now = OpenidConnect::now_unix_secs();
    let payload = OidcSessionCookie {
        access_token: "a".repeat(4096),
        created_at: now,
        session_expires_at: now + 3600,
        expires_at: Some(now + 60),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };
    let mut session = MockPluginSession::new();
    let captured = capture_appended_set_cookies(&mut session);
    let mut log = PluginLog::new("OpenidConnect");
    plugin
        .maybe_set_session_cookie_header(&mut session, &payload, 0, &mut log)
        .await;
    assert_eq!(plugin_log_entries(&log), vec!["err: session cookie too large"]);
    let mut expected = expected_logout_set_cookies();
    expected.pop();
    assert_eq!(*captured.lock(), expected);

    // Without a session credential nothing is published and nothing is logged.
    let mut missing = stateless_session_config();
    missing.resolved_session_secret = None;
    let plugin = OpenidConnect::new(&missing, "default".to_string());
    let mut session = MockPluginSession::new();
    session.expect_append_response_header().times(0);
    let mut log = PluginLog::new("OpenidConnect");
    plugin
        .maybe_set_session_cookie_header(&mut session, &payload, 0, &mut log)
        .await;
    assert!(plugin_log_entries(&log).is_empty());
}

#[tokio::test]
async fn fresh_plugin_instance_authenticates_from_the_cookie_without_refresh() {
    let mut cfg = stateless_session_config();
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.introspection_cache_ttl = 60;
    cfg.renew_access_token_on_expiry = true;
    // Instance A issues the cookie; instance B models another replica or plugin generation.
    let writer = OpenidConnect::new(&cfg, "default".to_string());
    let reader = OpenidConnect::new(&cfg, "default".to_string());
    // No token endpoint: any refresh attempt would fail and change the outcome.
    reader
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
    let token = "replica-independent-token";
    reader
        .cache_introspection_claims(
            token,
            &Arc::new(json!({"active": true, "iss": "https://idp.example.com", "exp": now + 3600})),
        )
        .await;
    let payload = OidcSessionCookie {
        access_token: token.to_string(),
        created_at: now,
        session_expires_at: now + 3600,
        expires_at: Some(writer.refresh_horizon(now, Some(3600))),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };
    let mut write_session = MockPluginSession::new();
    let written = capture_appended_set_cookies(&mut write_session);
    writer
        .maybe_set_session_cookie_header(&mut write_session, &payload, 0, &mut PluginLog::new("OpenidConnect"))
        .await;
    let encoded = set_cookie_value(&written.lock()[0], "edgion_oidc_session")
        .expect("session cookie")
        .to_string();

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("OpenidConnect");
    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    let cookies = install_request_cookies(
        &mut session,
        &format!("app=keep; edgion_oidc_session={encoded}; theme=dark"),
    );
    session.expect_append_response_header().times(0);

    let result = reader.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(plugin_log_entries(&log), vec!["session token verified"]);
    assert_eq!(cookies.lock().as_deref(), Some("app=keep; theme=dark"));
}

#[tokio::test]
async fn test_run_request_token_less_cookie_uses_refresh_singleflight_cached_result() {
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
            &Arc::new(json!({
                "active": true,
                "iss": "https://idp.example.com",
                "exp": now + 120
            })),
        )
        .await;

    let session_cookie_payload = OidcSessionCookie {
        access_token: String::new(),
        created_at: now,
        session_expires_at: now + 3600,
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
                access_token: refreshed_token.to_string(),
                created_at: now,
                session_expires_at: now + 3600,
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
    // The refreshed single cookie plus the chunk-count expiration.
    session
        .expect_append_response_header()
        .withf(|name, _| name.eq_ignore_ascii_case("Set-Cookie"))
        .times(2)
        .returning(|_, _| Ok(()));

    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(
        plugin_log_entries(&log),
        vec!["token refresh preemptive", "session token verified"]
    );
    assert!(cookies.lock().is_none());
}

#[tokio::test]
async fn refresh_from_a_chunked_cookie_expires_the_previous_chunks() {
    let mut cfg = stateless_session_config();
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.introspection_cache_ttl = 60;
    cfg.renew_access_token_on_expiry = true;
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
    plugin
        .cache_introspection_claims(
            "refreshed-token",
            &Arc::new(json!({"active": true, "iss": "https://idp.example.com", "exp": now + 3600})),
        )
        .await;
    let stale = OidcSessionCookie {
        access_token: "stale-token".into(),
        created_at: now,
        session_expires_at: now + 3600,
        // Refresh horizon reached.
        expires_at: Some(now),
        id_token: None,
        refresh_token: Some("refresh-token".into()),
    };
    let sealed = OpenidConnect::encode_signed_cookie_payload(&stale, plugin.session_cipher().unwrap()).unwrap();
    let mut refreshed = stale.clone();
    refreshed.access_token = "refreshed-token".into();
    refreshed.expires_at = Some(now + 3000);
    plugin
        .put_refresh_result(&OpenidConnect::refresh_lock_key(&sealed), refreshed)
        .await;
    // The browser holds the session as three chunks; the refreshed value fits one cookie.
    let third = sealed.len() / 3;
    let header = format!(
        "edgion_oidc_session_chunks=3; edgion_oidc_session_0={}; edgion_oidc_session_1={}; edgion_oidc_session_2={}",
        &sealed[..third],
        &sealed[third..2 * third],
        &sealed[2 * third..]
    );
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    install_request_cookies(&mut session, &header);
    let published = capture_appended_set_cookies(&mut session);
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        plugin_log_entries(&log),
        vec!["token refresh preemptive", "session token verified"]
    );
    let published = published.lock().clone();
    assert!(published[0].starts_with("edgion_oidc_session="));
    assert_eq!(
        published[1..],
        [
            expire_line("edgion_oidc_session_chunks"),
            expire_line("edgion_oidc_session_0"),
            expire_line("edgion_oidc_session_1"),
            expire_line("edgion_oidc_session_2"),
        ]
    );
}

#[tokio::test]
async fn ambiguous_session_cookies_are_rejected_with_a_static_tag() {
    let plugin = OpenidConnect::new(&stateless_session_config(), "default".to_string());
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    install_request_cookies(
        &mut session,
        "edgion_oidc_session=planted; edgion_oidc_session_chunks=2; edgion_oidc_session_0=a; edgion_oidc_session_1=b",
    );
    session.expect_append_response_header().times(0);
    super::dependency_error_tests::expect_auth_response(&mut session, 401, "Unauthorized - Authentication required");
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    assert_eq!(
        plugin_log_entries(&log),
        vec!["err: session cookie malformed", "no token; deny"]
    );
}

#[tokio::test]
async fn test_run_request_expired_refresh_publishes_updated_session_token() {
    let mut cfg = base_config();
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Deny;
    cfg.verification_mode = VerificationMode::IntrospectionOnly;
    cfg.introspection_cache_ttl = 60;
    cfg.renew_access_token_on_expiry = true;
    cfg.clock_skew_seconds = 0;
    cfg.access_token_expires_leeway = 0;
    cfg.set_access_token_header = true;
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
    // Bypass normal expiry clipping to exercise verification's expired verdict.
    plugin.introspection_cache.insert(
        OpenidConnect::introspection_cache_key("old-token"),
        IntrospectionCacheEntry {
            claims: Arc::new(json!({"active": true, "iss": "https://idp.example.com", "exp": now - 120})),
        },
        Duration::from_secs(60),
    );
    plugin
        .cache_introspection_claims(
            "new-token",
            &Arc::new(json!({"active": true, "iss": "https://idp.example.com", "exp": now + 3600})),
        )
        .await;
    let mut payload = OidcSessionCookie {
        access_token: "old-token".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
        // Keep preemptive refresh out of this verification-expiry scenario.
        expires_at: Some(now + 3600),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };
    let encoded = OpenidConnect::encode_signed_cookie_payload(&payload, plugin.session_cipher().unwrap()).unwrap();
    payload.access_token = "new-token".to_string();
    plugin
        .put_refresh_result(&OpenidConnect::refresh_lock_key(&encoded), payload)
        .await;
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/".to_string());
    session.expect_get_query_param().returning(|_| None);
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Access-Token" && value == "new-token" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));
    let cookies = install_request_cookies(&mut session, &format!("edgion_oidc_session={encoded}"));
    let published = capture_appended_set_cookies(&mut session);
    let mut log = PluginLog::new("OpenidConnect");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(plugin_log_entries(&log), vec!["token refresh on expiry"]);
    let published = published.lock().clone();
    assert_eq!(
        published.len(),
        2,
        "refreshed single cookie plus the chunk-count expiration"
    );
    let rewritten = set_cookie_value(&published[0], "edgion_oidc_session").expect("rewritten session cookie");
    let decoded: OidcSessionCookie =
        OpenidConnect::decode_signed_cookie_payload(rewritten, plugin.session_cipher().unwrap()).unwrap();
    assert_eq!(decoded.access_token, "new-token");
    assert!(cookies.lock().is_none());
}

#[tokio::test(start_paused = true)]
async fn test_run_request_token_less_cookie_waits_only_one_singleflight_timeout() {
    let mut cfg = base_config();
    cfg.timeout = 1;
    cfg.bearer_only = false;
    cfg.unauth_action = UnauthAction::Pass;
    cfg.renew_access_token_on_expiry = true;
    cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".to_string());
    let plugin = Arc::new(OpenidConnect::new(&cfg, "default".to_string()));

    let now = OpenidConnect::now_unix_secs();
    let payload = OidcSessionCookie {
        access_token: String::new(),
        created_at: now,
        session_expires_at: now + 3600,
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
        access_token: String::new(),
        created_at: now,
        session_expires_at: now + 3600,
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
                IntrospectionCacheEntry {
                    claims: Arc::new(claims),
                },
                Duration::from_secs(60),
            );
            let payload = OidcSessionCookie {
                access_token: token,
                created_at: now,
                session_expires_at: now + 3600,
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

    plugin.strip_gateway_owned_cookies(&mut session).unwrap();

    assert_eq!(cookies.lock().as_deref(), Some("app=keep; theme=dark"));
}

#[test]
fn strip_gateway_owned_cookies_drops_header_when_only_gateway_cookies() {
    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let mut session = MockPluginSession::new();
    let cookies = install_request_cookies(&mut session, "edgion_oidc_session=secret; edgion_oidc_state=state");

    plugin.strip_gateway_owned_cookies(&mut session).unwrap();

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

    plugin.strip_gateway_owned_cookies(&mut session).unwrap();

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
    let for_edit = Arc::clone(&cookie);
    session
        .expect_remove_request_cookies()
        .times(1)
        .returning(move |names| {
            assert_eq!(names, gateway_owned_cookie_names().as_slice());
            let mut headers = http::HeaderMap::new();
            headers.append(
                http::header::COOKIE,
                http::HeaderValue::from_static("edgion_oidc_session=secret; app=keep"),
            );
            headers.append(
                http::header::COOKIE,
                http::HeaderValue::from_static("edgion_oidc_state=state; theme=dark"),
            );
            let crate::request_cookie::CookieEditPlan::Replace(value) =
                crate::request_cookie::plan_removal(&headers, names)?
            else {
                panic!("expected survivors")
            };
            assert!(value.is_sensitive());
            *for_edit.lock() = Some(value.to_str().unwrap().to_owned());
            Ok(crate::request_cookie::CookieEditOutcome::Replaced)
        });

    plugin.strip_gateway_owned_cookies(&mut session).unwrap();

    assert_eq!(cookie.lock().as_deref(), Some("app=keep; theme=dark"));
}

#[test]
fn test_looks_like_jwt() {
    let jwtish = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.e30.signature";
    let opaque = "opaque-token-value";
    assert!(OpenidConnect::decode_jwt_header(jwtish).is_some());
    assert!(OpenidConnect::decode_jwt_header(opaque).is_none());
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
    // `expires_at` is the stored refresh horizon: reached now, so a refresh is due.
    let near_exp = OidcSessionCookie {
        access_token: "a".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
        expires_at: Some(now),
        id_token: None,
        refresh_token: Some("r".to_string()),
    };
    let far_exp = OidcSessionCookie {
        access_token: "a".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
        expires_at: Some(now + 1000),
        id_token: None,
        refresh_token: Some("r".to_string()),
    };
    let no_exp = OidcSessionCookie {
        expires_at: None,
        ..far_exp.clone()
    };
    // The leeway is folded into the horizon at write time; the check adds nothing.
    let horizon_ahead_within_leeway = OidcSessionCookie {
        expires_at: Some(now + 10),
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
    let cookie_token_blank = OidcSessionCookie {
        access_token: " \t".to_string(),
        ..far_exp.clone()
    };
    assert!(plugin.should_preemptive_refresh(&near_exp));
    assert!(!plugin.should_preemptive_refresh(&far_exp));
    assert!(plugin.should_preemptive_refresh(&cookie_token_missing));
    assert!(plugin.should_preemptive_refresh(&cookie_token_blank));
    assert!(!plugin.should_preemptive_refresh(&no_exp));
    assert!(!plugin.should_preemptive_refresh(&horizon_ahead_within_leeway));
    assert!(!plugin.should_preemptive_refresh(&no_refresh));

    // The knob also gates the token-less case (formerly an unconditional cache-miss refresh).
    let mut disabled_cfg = cfg;
    disabled_cfg.renew_access_token_on_expiry = false;
    let disabled = OpenidConnect::new(&disabled_cfg, "default".to_string());
    assert!(!disabled.should_preemptive_refresh(&near_exp));
    assert!(!disabled.should_preemptive_refresh(&cookie_token_missing));
}

#[test]
fn refresh_horizon_clamps_the_clock_skew_margin() {
    let mut cfg = base_config();
    cfg.clock_skew_seconds = 120;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    // Long-lived token: the full clock-skew margin applies.
    assert_eq!(plugin.refresh_horizon(1_000, Some(3600)), 1_000 + 3600 - 120);
    // Missing expires_in falls back to 300 s.
    assert_eq!(plugin.refresh_horizon(1_000, None), 1_000 + 300 - 120);
    // A token shorter than the clock skew keeps half its lifetime instead of refreshing
    // on every request.
    assert_eq!(plugin.refresh_horizon(1_000, Some(60)), 1_000 + 30);
    assert_eq!(plugin.refresh_horizon(1_000, Some(1)), 1_000 + 1);
    // An IdP-reported zero lifetime uses the fallback instead of refreshing every request.
    assert_eq!(plugin.refresh_horizon(1_000, Some(0)), 1_000 + 300 - 120);
    assert_eq!(plugin.refresh_horizon(u64::MAX, Some(10)), u64::MAX);
    let mut no_skew = base_config();
    no_skew.clock_skew_seconds = 0;
    let plugin = OpenidConnect::new(&no_skew, "default".to_string());
    assert_eq!(plugin.refresh_horizon(1_000, Some(60)), 1_060);
    // The leeway is the early-refresh margin when larger than the skew, never their sum,
    // and is clamped to half the lifetime.
    let mut leeway = base_config();
    leeway.clock_skew_seconds = 120;
    leeway.access_token_expires_leeway = 300;
    let plugin = OpenidConnect::new(&leeway, "default".to_string());
    assert_eq!(plugin.refresh_horizon(1_000, Some(3600)), 1_000 + 3600 - 300);
    assert_eq!(plugin.refresh_horizon(1_000, Some(400)), 1_000 + 200);
    assert_eq!(plugin.refresh_horizon(1_000, None), 1_000 + 150);
}

fn token_response(value: serde_json::Value) -> super::TokenEndpointResponse {
    serde_json::from_value(value).unwrap()
}

#[test]
fn refresh_response_rejects_token_less_documents_and_keeps_the_session() {
    let plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let original = OidcSessionCookie {
        access_token: "old-token".into(),
        created_at: 7,
        session_expires_at: 7 + 3600,
        expires_at: Some(42),
        id_token: Some("old-id".into()),
        refresh_token: Some("old-refresh".into()),
    };
    for token in ["", " \t"] {
        let mut payload = original.clone();
        let result = plugin.apply_refresh_response(
            &mut payload,
            token_response(json!({"access_token": token, "refresh_token": "rotated", "expires_in": 600})),
            true,
            1_000,
        );
        assert!(matches!(
            result,
            Err(super::OidcVerifyError::Dependency(error))
                if error.operation == super::OidcOperation::Refresh
                    && error.failure == super::OidcDependencyFailure::InvalidDocument
        ));
        assert_eq!(
            serde_json::to_value(&payload).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
    }
}

#[test]
fn refresh_response_persists_token_horizon_and_rotation() {
    let mut cfg = base_config();
    cfg.clock_skew_seconds = 120;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let base = OidcSessionCookie {
        access_token: "old-token".into(),
        created_at: 7,
        session_expires_at: 7 + 3600,
        expires_at: Some(42),
        id_token: Some("old-id".into()),
        refresh_token: Some("old-refresh".into()),
    };
    let mut payload = base.clone();
    plugin
        .apply_refresh_response(
            &mut payload,
            token_response(json!({"access_token": "new-token", "id_token": "new-id", "refresh_token": "rotated", "expires_in": 600})),
            true,
            1_000,
        )
        .unwrap();
    assert_eq!(payload.access_token, "new-token");
    assert_eq!(payload.expires_at, Some(1_000 + 600 - 120));
    assert_eq!(payload.id_token.as_deref(), Some("new-id"));
    assert_eq!(payload.refresh_token.as_deref(), Some("rotated"));
    assert_eq!(payload.session_expires_at, base.session_expires_at);

    // Missing expires_in still yields a horizon; absent rotation keeps the refresh token;
    // the ID token is untouched when it is not persisted.
    let mut payload = base.clone();
    plugin
        .apply_refresh_response(
            &mut payload,
            token_response(json!({"access_token": "new-token"})),
            false,
            1_000,
        )
        .unwrap();
    assert_eq!(payload.expires_at, Some(1_000 + 300 - 120));
    assert_eq!(payload.refresh_token.as_deref(), Some("old-refresh"));
    assert_eq!(payload.id_token.as_deref(), Some("old-id"));
}

#[test]
fn callback_session_payload_carries_the_access_token_and_horizon() {
    let mut cfg = stateless_session_config();
    cfg.clock_skew_seconds = 120;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let payload = plugin.new_session_payload(
        token_response(
            json!({"access_token": "login-token", "id_token": "id", "refresh_token": "r", "expires_in": 3600}),
        ),
        1_000,
        999,
        999 + 3600,
        true,
    );
    assert_eq!(payload.access_token, "login-token");
    assert_eq!(payload.expires_at, Some(1_000 + 3600 - 120));
    assert_eq!(payload.created_at, 999);
    assert_eq!(payload.session_expires_at, 999 + 3600);
    assert_eq!(payload.id_token.as_deref(), Some("id"));
    assert_eq!(payload.refresh_token.as_deref(), Some("r"));
    // The sealed callback cookie decodes back with the token included.
    let sealed = OpenidConnect::encode_signed_cookie_payload(&payload, plugin.session_cipher().unwrap()).unwrap();
    let decoded: OidcSessionCookie =
        OpenidConnect::decode_signed_cookie_payload(&sealed, plugin.session_cipher().unwrap()).unwrap();
    assert_eq!(decoded.access_token, "login-token");

    let without_id = plugin.new_session_payload(
        token_response(json!({"access_token": "login-token", "id_token": "id"})),
        1_000,
        999,
        999 + 3600,
        false,
    );
    assert_eq!(without_id.id_token, None);
    assert_eq!(without_id.expires_at, Some(1_000 + 300 - 120));
    let empty_id = plugin.new_session_payload(
        token_response(json!({"access_token": "login-token", "id_token": ""})),
        1_000,
        999,
        999 + 3600,
        true,
    );
    assert_eq!(empty_id.id_token, None);
}

#[test]
fn callback_cookies_expire_stale_chunks_then_the_state_cookie() {
    let plugin = chunking_plugin(3800, 8);
    let session = session_with_cookie_header(
        "edgion_oidc_session_chunks=3; edgion_oidc_session_0=a; edgion_oidc_session_1=b; edgion_oidc_session_2=c",
    );
    let cookies = plugin.callback_set_cookies(&session, "sealed.value", 3600).unwrap();
    assert_eq!(
        cookies,
        vec![
            "edgion_oidc_session=sealed.value; Path=/; SameSite=Lax; Max-Age=3600; HttpOnly; Secure".to_string(),
            expire_line("edgion_oidc_session_chunks"),
            expire_line("edgion_oidc_session_0"),
            expire_line("edgion_oidc_session_1"),
            expire_line("edgion_oidc_session_2"),
            "edgion_oidc_state=; Path=/.edgion/oidc/callback; SameSite=Lax; Max-Age=0; HttpOnly; Secure".to_string(),
        ]
    );
    // A session that cannot fit is an error; the callback answers with its fixed 500.
    let small = chunking_plugin(512, 1);
    let session = session_with_cookie_header("");
    assert!(small.callback_set_cookies(&session, &"x".repeat(600), 3600).is_err());
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

    let claims = Arc::new(json!({"active": true, "sub": "u1"}));
    plugin.cache_introspection_claims("token-a", &claims).await;

    assert_eq!(
        plugin.get_cached_introspection_claims("token-a").await,
        Some(claims.clone())
    );
    assert_eq!(plugin.get_cached_introspection_claims("token-b").await, None);
}

#[tokio::test]
async fn introspection_cache_hits_share_claims_without_reducing_weight() {
    use crate::cache::LogicalWeight;

    let mut cfg = base_config();
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let claims = Arc::new(json!({"active": true, "sub": "u1", "groups": ["a", "b"]}));
    plugin.cache_introspection_claims("token-a", &claims).await;

    let first = plugin.get_cached_introspection_claims("token-a").await.unwrap();
    let second = plugin.get_cached_introspection_claims("token-a").await.unwrap();
    assert!(Arc::ptr_eq(&first, &claims));
    assert!(Arc::ptr_eq(&first, &second));

    let entry = IntrospectionCacheEntry { claims };
    // Root value, Arc counters and the reachable tree, so the byte budget does not shrink.
    assert_eq!(
        entry.logical_weight(),
        std::mem::size_of::<serde_json::Value>() + 2 * std::mem::size_of::<usize>() + entry.claims.logical_weight()
    );
}

#[tokio::test]
async fn zero_ttl_introspection_result_neither_inserts_nor_replaces_live_claims() {
    let mut config = base_config();
    config.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&config, "default".to_string());
    let expired = Arc::new(json!({"active": true, "exp": 0, "sub": "expired"}));
    plugin.cache_introspection_claims("same-token", &expired).await;
    assert!(plugin.get_cached_introspection_claims("same-token").await.is_none());

    let live = Arc::new(json!({"active": true, "sub": "live"}));
    plugin.cache_introspection_claims("same-token", &live).await;
    plugin.cache_introspection_claims("same-token", &expired).await;
    assert_eq!(plugin.get_cached_introspection_claims("same-token").await, Some(live));
}

#[test]
fn zero_ttl_introspection_skips_the_discarded_claims_copy() {
    use futures::FutureExt;

    async fn previous_insert(plugin: &OpenidConnect, token: &str, claims: &serde_json::Value) {
        if plugin.config.introspection_cache_ttl == 0 || claims.get("active") != Some(&serde_json::Value::Bool(true)) {
            return;
        }
        let key = OpenidConnect::introspection_cache_key(token);
        let ttl = OpenidConnect::bounded_introspection_cache_ttl(
            claims,
            plugin.config.introspection_cache_ttl,
            OpenidConnect::now_unix_secs(),
        );
        plugin.introspection_cache.insert(
            key,
            IntrospectionCacheEntry {
                claims: Arc::new(claims.clone()),
            },
            ttl,
        );
    }

    let mut config = base_config();
    config.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&config, "default".to_string());
    let claims = Arc::new(json!({
        "active": true, "exp": 0,
        "details": (0..64).map(|index| json!({"name": format!("claim-{index}"), "value": "payload".repeat(32)})).collect::<Vec<_>>()
    }));
    let (old_done, old_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        previous_insert(&plugin, "expired-token", &claims).now_or_never()
    });
    let (new_done, new_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        plugin
            .cache_introspection_claims("expired-token", &claims)
            .now_or_never()
    });
    assert_eq!(old_done, Some(()));
    assert_eq!(new_done, Some(()));
    assert!(new_stats.calls < old_stats.calls);
    assert!(new_stats.requested_bytes < old_stats.requested_bytes);
    let key = OpenidConnect::introspection_cache_key("expired-token");
    assert!(plugin.introspection_cache.get(&key).0.is_none());
}

#[tokio::test]
async fn inactive_introspection_result_is_never_cached() {
    let mut cfg = base_config();
    cfg.introspection_cache_ttl = 60;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());

    plugin
        .cache_introspection_claims("inactive-token", &Arc::new(json!({"active": false, "sub": "u1"})))
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
            claims: Arc::new(json!({"active": true, "sub": "old"})),
        },
        StdDuration::from_millis(1),
    );
    // Wait for it to expire
    tokio::time::sleep(StdDuration::from_millis(10)).await;

    plugin
        .cache_introspection_claims("fresh-token", &Arc::new(json!({"active": true, "sub": "new"})))
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
    let claims = Arc::new(json!({
        "active": true,
        "iss": "https://issuer.example.com",
        "exp": now + 120
    }));
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
        access_token: "old-token".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
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
                access_token: "cached-token".to_string(),
                created_at: now,
                session_expires_at: now + 3600,
                expires_at: Some(now + 30),
                id_token: Some("id-token".to_string()),
                refresh_token: Some("refresh-token".to_string()),
            },
        )
        .await;

    let flight = plugin.refresh_singleflight.get_or_create(lock_key);
    let _held_guard = flight.lock().lock().await;

    let mut cookie = OidcSessionCookie {
        access_token: "old-token".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
        expires_at: Some(now + 10),
        id_token: None,
        refresh_token: Some("refresh-token".to_string()),
    };

    let result = plugin
        .try_refresh_session_token_singleflight(lock_key, &mut cookie)
        .await;
    assert_eq!(result, Ok(()));
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
        access_token: "cached-token".to_string(),
        created_at: now,
        session_expires_at: now + 3600,
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

fn chunking_plugin(max_bytes: u64, max_chunks: u32) -> OpenidConnect {
    let mut cfg = base_config();
    cfg.max_session_cookie_bytes = max_bytes;
    cfg.max_session_cookie_chunks = max_chunks;
    OpenidConnect::new(&cfg, "default".to_string())
}

fn expire_line(name: &str) -> String {
    format!("{name}=; Path=/; SameSite=Lax; Max-Age=0; HttpOnly; Secure")
}

/// Mock session whose request carries exactly `cookie_header`.
fn session_with_cookie_header(cookie_header: &str) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    if cookie_header.is_empty() {
        setup_header_mocks(&mut session, &[]);
    } else {
        setup_header_mocks(&mut session, &[("cookie", cookie_header)]);
    }
    session
}

/// Turn written Set-Cookie lines into the Cookie header a browser would send back.
fn cookie_header_from_set_cookies(cookies: &[String]) -> String {
    cookies
        .iter()
        .filter(|cookie| !cookie.contains("Max-Age=0"))
        .filter_map(|cookie| cookie.split(';').next())
        .collect::<Vec<_>>()
        .join("; ")
}

#[test]
fn session_cookie_fits_in_a_single_cookie_and_expires_stale_chunks() {
    let plugin = chunking_plugin(3800, 8);
    let cookies = plugin.build_session_set_cookies("sealed.value", 3600, 3).unwrap();
    assert_eq!(
        cookies,
        vec![
            "edgion_oidc_session=sealed.value; Path=/; SameSite=Lax; Max-Age=3600; HttpOnly; Secure".to_string(),
            expire_line("edgion_oidc_session_chunks"),
            expire_line("edgion_oidc_session_0"),
            expire_line("edgion_oidc_session_1"),
            expire_line("edgion_oidc_session_2"),
        ]
    );
    // A request without chunks still expires the count cookie (last writer wins).
    let cookies = plugin.build_session_set_cookies("sealed.value", 3600, 0).unwrap();
    assert_eq!(cookies.len(), 2);
    assert_eq!(cookies[1], expire_line("edgion_oidc_session_chunks"));
}

#[test]
fn oversized_session_cookie_is_chunked_within_the_limit_and_round_trips() {
    let max_bytes = 512;
    let plugin = chunking_plugin(max_bytes, 8);
    let sealed: String = (0..2000).map(|index| char::from(b'a' + (index % 26) as u8)).collect();
    let cookies = plugin.build_session_set_cookies(&sealed, 3600, 0).unwrap();
    let written: Vec<&String> = cookies.iter().filter(|cookie| !cookie.contains("Max-Age=0")).collect();
    let chunk_count = written.len() - 1;
    assert!(chunk_count >= 2);
    for (index, cookie) in written[..chunk_count].iter().enumerate() {
        assert!(cookie.starts_with(&format!("edgion_oidc_session_{index}=")));
        assert!(cookie.len() <= max_bytes as usize, "chunk {index} exceeds the limit");
    }
    let count_cookie = written[chunk_count];
    assert_eq!(
        *count_cookie,
        format!("edgion_oidc_session_chunks={chunk_count}; Path=/; SameSite=Lax; Max-Age=3600; HttpOnly; Secure")
    );
    assert!(count_cookie.len() <= max_bytes as usize);
    // The single cookie is always expired by a chunked write.
    assert_eq!(cookies.last().unwrap(), &expire_line("edgion_oidc_session"));

    let header = cookie_header_from_set_cookies(&cookies);
    let session = session_with_cookie_header(&header);
    let value = plugin.read_session_cookie_value(&session).unwrap().unwrap();
    assert_eq!(value.raw, sealed);
    assert_eq!(value.prior_chunks, chunk_count);
}

#[test]
fn chunked_rewrite_expires_only_stale_higher_indexes() {
    let plugin = chunking_plugin(512, 8);
    let sealed = "x".repeat(700);
    let cookies = plugin.build_session_set_cookies(&sealed, 60, 5).unwrap();
    let expired: Vec<&String> = cookies.iter().filter(|cookie| cookie.contains("Max-Age=0")).collect();
    assert_eq!(
        expired,
        vec![
            &expire_line("edgion_oidc_session"),
            &expire_line("edgion_oidc_session_2"),
            &expire_line("edgion_oidc_session_3"),
            &expire_line("edgion_oidc_session_4"),
        ]
    );
}

#[test]
fn session_cookie_write_limits_fail_closed_without_underflow() {
    // The chunk overhead alone exceeds the limit: an error, never a wrapped capacity.
    assert!(chunking_plugin(64, 8)
        .build_session_set_cookies(&"x".repeat(100), 3600, 0)
        .is_err());
    assert!(chunking_plugin(1, 8).build_session_set_cookies("xy", 3600, 0).is_err());
    // Needing more chunks than configured fails; `1` disables chunking.
    assert!(chunking_plugin(512, 2)
        .build_session_set_cookies(&"x".repeat(2000), 3600, 0)
        .is_err());
    assert!(chunking_plugin(512, 1)
        .build_session_set_cookies(&"x".repeat(600), 3600, 0)
        .is_err());
    assert!(chunking_plugin(512, 1)
        .build_session_set_cookies("small", 3600, 0)
        .is_ok());
}

#[test]
fn malformed_or_ambiguous_chunk_sets_fail_closed() {
    let plugin = chunking_plugin(512, 8);
    let cases = [
        // Missing chunk 1.
        "edgion_oidc_session_chunks=2; edgion_oidc_session_0=aa",
        // Count below two, above the reader cap, or not a number.
        "edgion_oidc_session_chunks=1; edgion_oidc_session_0=aa",
        "edgion_oidc_session_chunks=17; edgion_oidc_session_0=aa; edgion_oidc_session_1=bb",
        "edgion_oidc_session_chunks=x; edgion_oidc_session_0=aa; edgion_oidc_session_1=bb",
    ];
    for header in cases {
        let session = session_with_cookie_header(header);
        assert!(
            matches!(
                plugin.read_session_cookie_value(&session),
                Err(super::OidcVerifyError::Auth(super::AuthFailure::MalformedCookie))
            ),
            "{header}"
        );
        // No fallback: a planted count/chunk cookie must not select the single cookie.
        let with_single = format!("{header}; edgion_oidc_session=single");
        let session = session_with_cookie_header(&with_single);
        assert!(
            matches!(
                plugin.read_session_cookie_value(&session),
                Err(super::OidcVerifyError::Auth(super::AuthFailure::MalformedCookie))
            ),
            "{with_single}"
        );
    }
    // The reassembled length is capped at 16 x maxSessionCookieBytes.
    let small = chunking_plugin(1, 8);
    let session = session_with_cookie_header(
        "edgion_oidc_session_chunks=2; edgion_oidc_session_0=aaaaaaaaaaaaaaaa; edgion_oidc_session_1=bbbbbbbbbbbbbbbbb",
    );
    assert!(matches!(
        small.read_session_cookie_value(&session),
        Err(super::OidcVerifyError::Auth(super::AuthFailure::MalformedCookie))
    ));
    // Exactly 16 x maxSessionCookieBytes is accepted.
    let session = session_with_cookie_header(&format!(
        "edgion_oidc_session_chunks=2; edgion_oidc_session_0={}; edgion_oidc_session_1={}",
        "a".repeat(8),
        "b".repeat(8)
    ));
    assert_eq!(
        small.read_session_cookie_value(&session).unwrap().unwrap().raw.len(),
        16
    );
    // The callback still learns the parsed count of a malformed set to expire stale chunks.
    let session = session_with_cookie_header("edgion_oidc_session_chunks=3; edgion_oidc_session=single");
    assert_eq!(plugin.prior_session_chunks(&session), 3);
    let session = session_with_cookie_header("edgion_oidc_session_chunks=99");
    assert_eq!(plugin.prior_session_chunks(&session), 16);
    // No session cookies at all is not an error; a lone single cookie is the session.
    let session = session_with_cookie_header("app=keep");
    assert!(plugin.read_session_cookie_value(&session).unwrap().is_none());
    let session = session_with_cookie_header("edgion_oidc_session=single");
    let value = plugin.read_session_cookie_value(&session).unwrap().unwrap();
    assert_eq!((value.raw.as_str(), value.prior_chunks), ("single", 0));
    // A valid chunk set reports its count; alongside a leftover single cookie it fails closed.
    let session =
        session_with_cookie_header("edgion_oidc_session_chunks=2; edgion_oidc_session_0=aa; edgion_oidc_session_1=bb");
    let value = plugin.read_session_cookie_value(&session).unwrap().unwrap();
    assert_eq!((value.raw.as_str(), value.prior_chunks), ("aabb", 2));
    let session = session_with_cookie_header(
        "edgion_oidc_session=single; edgion_oidc_session_chunks=2; edgion_oidc_session_0=aa; edgion_oidc_session_1=bb",
    );
    assert!(plugin.read_session_cookie_value(&session).is_err());
    // Jars that keep expired cookies as empty values must not lock the session out.
    let session = session_with_cookie_header("edgion_oidc_session=single; edgion_oidc_session_chunks=");
    assert_eq!(
        plugin.read_session_cookie_value(&session).unwrap().unwrap().raw,
        "single"
    );
    let session = session_with_cookie_header(
        "edgion_oidc_session=; edgion_oidc_session_chunks=2; edgion_oidc_session_0=aa; edgion_oidc_session_1=bb",
    );
    assert_eq!(plugin.read_session_cookie_value(&session).unwrap().unwrap().raw, "aabb");
    let session = session_with_cookie_header("edgion_oidc_session=; edgion_oidc_session_chunks=");
    assert!(plugin.read_session_cookie_value(&session).unwrap().is_none());
    assert_eq!(plugin.prior_session_chunks(&session), 0);
}

#[test]
fn every_written_session_line_fits_the_limit_or_the_write_fails() {
    // Property over limits and value sizes: a successful write never emits a line above
    // the limit (this pins the `_chunks` worst-case overhead), and chunking stays within
    // the configured count, including the equality boundary.
    for max_bytes in (40..=400).step_by(3) {
        for len in [1usize, 6, 7, 50, 150, 700] {
            let plugin = chunking_plugin(max_bytes, 16);
            let sealed = "s".repeat(len);
            if let Ok(cookies) = plugin.build_session_set_cookies(&sealed, 3600, 0) {
                for cookie in cookies.iter().filter(|cookie| !cookie.contains("Max-Age=0")) {
                    assert!(cookie.len() <= max_bytes as usize, "{max_bytes}/{len}: {cookie}");
                }
                let header = cookie_header_from_set_cookies(&cookies);
                let session = session_with_cookie_header(&header);
                assert_eq!(plugin.read_session_cookie_value(&session).unwrap().unwrap().raw, sealed);
            }
        }
    }
    let exact = chunking_plugin(512, 2);
    let cookies = exact.build_session_set_cookies(&"x".repeat(700), 3600, 0).unwrap();
    assert!(cookies
        .iter()
        .any(|cookie| cookie.starts_with("edgion_oidc_session_chunks=2;")));
}

#[test]
fn stale_chunk_expiry_is_clamped_to_the_reader_cap() {
    let plugin = chunking_plugin(3800, 8);
    let cookies = plugin.build_session_set_cookies("small", 3600, 99).unwrap();
    assert_eq!(cookies.len(), 2 + 16);
    assert_eq!(cookies.last().unwrap(), &expire_line("edgion_oidc_session_15"));
}

#[test]
fn chunk_names_follow_the_configured_session_cookie_name() {
    let mut cfg = base_config();
    cfg.session_cookie_name = "custom".to_string();
    cfg.max_session_cookie_bytes = 256;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let cookies = plugin.build_session_set_cookies(&"v".repeat(600), 60, 0).unwrap();
    assert!(cookies[0].starts_with("custom_0="));
    assert!(cookies.iter().any(|cookie| cookie.starts_with("custom_chunks=")));
    let mut session = MockPluginSession::new();
    let remaining = install_request_cookies(
        &mut session,
        "custom_chunks=2; custom_0=a; custom_1=b; edgion_oidc_session_0=app",
    );
    plugin.strip_gateway_owned_cookies(&mut session).unwrap();
    assert_eq!(remaining.lock().as_deref(), Some("edgion_oidc_session_0=app"));
}

#[test]
fn strip_gateway_owned_cookies_removes_every_chunk_name() {
    let plugin = chunking_plugin(3800, 8);
    let mut session = MockPluginSession::new();
    let cookies = install_request_cookies(
        &mut session,
        "app=keep; edgion_oidc_session_chunks=2; edgion_oidc_session_0=aa; edgion_oidc_session_15=zz; edgion_oidc_session_1=bb; edgion_oidc_state=s; edgion_oidc_session_16=app",
    );
    plugin.strip_gateway_owned_cookies(&mut session).unwrap();
    assert_eq!(cookies.lock().as_deref(), Some("app=keep; edgion_oidc_session_16=app"));
}

#[test]
fn test_cookie_attr_suffix_respects_cookie_config() {
    let mut cfg = base_config();
    cfg.session_cookie_same_site = "Strict".to_string();
    cfg.session_cookie_http_only = false;
    cfg.session_cookie_secure = false;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    assert_eq!(
        plugin.build_cookie_header_value("session", "fixture", 60, "/"),
        "session=fixture; Path=/; SameSite=Strict; Max-Age=60"
    );
}

#[test]
fn test_cookie_attr_suffix_sets_secure_when_enabled() {
    let mut cfg = base_config();
    cfg.session_cookie_secure = true;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    assert_eq!(
        plugin.build_cookie_header_value("session", "fixture", 60, "/"),
        "session=fixture; Path=/; SameSite=Lax; Max-Age=60; HttpOnly; Secure"
    );
}

// Frozen pre-oidc-04 composition, deliberately independent of the new formatter.
fn old_cookie_header_value(config: &OpenidConnectConfig, name: &str, value: &str, max_age: u64, path: &str) -> String {
    let secure = if config.session_cookie_secure { "; Secure" } else { "" };
    let http_only = if config.session_cookie_http_only {
        "; HttpOnly"
    } else {
        ""
    };
    let same_site = if config.session_cookie_same_site.eq_ignore_ascii_case("strict") {
        "Strict"
    } else if config.session_cookie_same_site.eq_ignore_ascii_case("none") {
        "None"
    } else {
        "Lax"
    };
    let suffix = format!(
        "; Path={}; SameSite={}; Max-Age={}{}{}",
        path, same_site, max_age, http_only, secure
    );
    format!("{}={}{}", name, value, suffix)
}

#[test]
fn cookie_header_formatting_matches_original_full_bytes() {
    let mut plugin = OpenidConnect::new(&base_config(), "default".to_string());
    let long_path = format!("/callback/{}", "segment/".repeat(128));
    let long_value = "a".repeat(3800);
    // Mutating the private fixture config intentionally exercises defensive
    // formatting too; it does not claim invalid configurations pass admission.
    for same_site in ["Lax", "Strict", "None", "sTrIcT", "nOnE", "", "unknown", " Strict"] {
        for secure in [false, true] {
            for http_only in [false, true] {
                plugin.config.session_cookie_same_site = same_site.into();
                plugin.config.session_cookie_secure = secure;
                plugin.config.session_cookie_http_only = http_only;
                for (name, value, max_age, path) in [
                    ("edgion_oidc_state", "fixture.state", 300, "/auth/callback"),
                    ("edgion_oidc_state", "", 0, "/auth/callback"),
                    ("custom_session", "fixture.session", plugin.config.session_lifetime, "/"),
                    ("custom_session", "", 0, "/"),
                    ("", "", u64::MAX, ""),
                    ("session", long_value.as_str(), u64::MAX, long_path.as_str()),
                    ("session", "é=%3B", 1, "/café"),
                    ("bad;name", "bad\r\nvalue", 2, "/bad;path"),
                ] {
                    let actual = plugin.build_cookie_header_value(name, value, max_age, path);
                    assert_eq!(
                        actual,
                        old_cookie_header_value(&plugin.config, name, value, max_age, path)
                    );
                    assert_eq!(actual.capacity(), actual.len());
                }
            }
        }
    }
}

#[test]
fn cookie_header_formatting_sizes_decimal_boundaries() {
    let plugin = OpenidConnect::new(&base_config(), "default".into());
    let mut max_ages = vec![0, u64::MAX];
    for exponent in 0..=19 {
        let power = 10u64.pow(exponent);
        max_ages.extend([power - 1, power, power + 1]);
    }
    for max_age in max_ages {
        let actual = plugin.build_cookie_header_value("session", "fixture", max_age, "/callback");
        assert_eq!(
            actual,
            old_cookie_header_value(&plugin.config, "session", "fixture", max_age, "/callback"),
        );
        assert_eq!(actual.capacity(), actual.len(), "Max-Age {max_age}");
    }
}

#[test]
fn cookie_header_formatting_keeps_plugin_generations_independent() {
    let mut config = base_config();
    config.session_cookie_same_site = "Strict".into();
    config.session_cookie_http_only = false;
    let old = OpenidConnect::new(&config, "default".into());
    config.session_cookie_same_site = "Lax".into();
    config.session_cookie_secure = false;
    config.session_cookie_http_only = true;
    let new = OpenidConnect::new(&config, "default".into());
    for _ in 0..3 {
        assert_eq!(
            old.build_cookie_header_value("session", "fixture", 300, "/callback"),
            "session=fixture; Path=/callback; SameSite=Strict; Max-Age=300; Secure"
        );
        assert_eq!(
            new.build_cookie_header_value("session", "fixture", 0, "/"),
            "session=fixture; Path=/; SameSite=Lax; Max-Age=0; HttpOnly"
        );
    }
}

#[test]
fn cookie_header_formatting_allocation_comparison() {
    let plugin = OpenidConnect::new(&base_config(), "default".into());
    let long_path = format!("/callback/{}", "segment/".repeat(128));
    let session_value = "a".repeat(3800);
    for (case, name, value, max_age, path) in [
        ("clear", "session", "", 0, "/"),
        ("state", "edgion_oidc_state", "fixture.state", 300, "/callback"),
        (
            "long-path",
            "edgion_oidc_state",
            "fixture.state",
            300,
            long_path.as_str(),
        ),
        (
            "session",
            "session",
            session_value.as_str(),
            plugin.config.session_lifetime,
            "/",
        ),
    ] {
        let (baseline, before) = crate::test_allocation_counter::count_allocation_stats(|| {
            old_cookie_header_value(&plugin.config, name, value, max_age, path)
        });
        let (candidate, after) = crate::test_allocation_counter::count_allocation_stats(|| {
            plugin.build_cookie_header_value(name, value, max_age, path)
        });
        assert_eq!(candidate, baseline);
        // Only synthetic case labels, sizes and allocator statistics are emitted.
        // Allocation-call counts are measured rather than assumed.
        println!(
            "cookie-format {case}: baseline calls={} requested={} capacity={}; candidate calls={} requested={} capacity={}; len={}",
            before.calls, before.requested_bytes, baseline.capacity(),
            after.calls, after.requested_bytes, candidate.capacity(), candidate.len()
        );
    }
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

    plugin
        .apply_upstream_headers(
            &mut session,
            "access-token",
            Some("id-token"),
            Some("{\"sub\":\"userinfo-user\"}"),
            &json!({"sub": "user-1"}),
        )
        .unwrap();
}

#[test]
fn test_apply_upstream_headers_skips_oversized_id_token_header() {
    let mut cfg = base_config();
    cfg.set_id_token_header = true;
    cfg.max_header_value_bytes = 4;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    session.expect_set_request_header_value().never();
    plugin
        .apply_upstream_headers(
            &mut session,
            "access-token",
            Some("id-token-too-long"),
            None,
            &json!({"sub": "user-1"}),
        )
        .unwrap();
}

#[test]
fn test_apply_upstream_headers_rejects_unsafe_userinfo_header_value() {
    let mut cfg = base_config();
    cfg.set_userinfo_header = true;
    let plugin = OpenidConnect::new(&cfg, "default".to_string());
    let mut session = MockPluginSession::new();

    session.expect_set_request_header_value().never();
    plugin
        .apply_upstream_headers(
            &mut session,
            "access-token",
            None,
            Some("{\"sub\":\"user\n1\"}"),
            &json!({"sub": "user-1"}),
        )
        .unwrap();
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
    session.expect_remove_request_header().returning(|_| Ok(()));
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

    plugin
        .apply_upstream_headers(&mut session, "a", None, None, &json!({"sub": "u"}))
        .unwrap();
}

#[test]
fn authorization_length_limits_preserve_publication_order_and_remaining_budget() {
    // token, value cap, total cap, prior header, prior write fails, auth, projection
    for (token, value_cap, total_cap, prior, prior_fails, auth, projection) in [
        ("", 6, 20, false, false, false, true),
        ("", 7, 20, false, false, true, false),
        ("abc", 9, 23, false, false, false, true),
        ("abc", 10, 23, false, false, true, false),
        ("abc", 10, 22, false, false, false, true),
        ("abc", 10, 40, true, false, true, false),
        ("abc", 10, 39, true, false, false, true),
        ("abc", 10, 46, true, false, true, true),
        ("abc", 10, 23, true, true, true, false),
        ("abc", 10, 22, true, true, false, true),
    ] {
        let mut cfg = base_config();
        cfg.access_token_in_authorization_header = true;
        cfg.set_access_token_header = prior;
        cfg.store_claims_in_ctx = true;
        cfg.claims_to_headers = Some(HashMap::from([("sub".to_string(), "X-Sub".to_string())]));
        cfg.max_header_value_bytes = value_cap;
        cfg.max_total_header_bytes = total_cap;
        let plugin = OpenidConnect::new(&cfg, "default".to_string());
        let mut session = MockPluginSession::new();
        session.expect_remove_request_header().returning(|_| Ok(()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let header_events = events.clone();
        session.expect_set_request_header_value().returning(move |name, value| {
            assert!(value.is_sensitive());
            header_events.lock().push(format!("{name}={}", value.to_str().unwrap()));
            if name == "X-Access-Token" && prior_fails {
                Err(std::io::Error::other("injected write failure").into())
            } else {
                Ok(())
            }
        });
        let ctx_events = events.clone();
        session
            .expect_set_ctx_var()
            .withf(|name, value| name == "oidc_claims" && value == "{\"sub\":\"u\"}")
            .times(1)
            .returning(move |_, _| {
                ctx_events.lock().push("ctx".to_string());
                Ok(())
            });
        plugin
            .apply_upstream_headers(&mut session, token, None, None, &json!({"sub": "u"}))
            .unwrap();
        let mut expected = Vec::new();
        if prior {
            expected.push(format!("X-Access-Token={token}"));
        }
        if auth {
            expected.push(format!("Authorization=Bearer {token}"));
        }
        expected.push("ctx".to_string());
        if projection {
            expected.push("x-sub=u".to_string());
        }
        assert_eq!(
            *events.lock(),
            expected,
            "caps={value_cap}/{total_cap}, prior={prior}/{prior_fails}"
        );
    }
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
        ResolverSpec::StandardDns(edgion_common::dns::StandardDnsServers::new(vec![
            "not-a-dns-server".to_string()
        ])),
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
async fn auto_header_routing_preserves_jwt_failures_and_introspection_choice() {
    use super::super::error::{AuthFailure, OidcVerifyError};
    use super::super::jwks_provider::{ScriptedHttp, JWKS_PATH};

    let claims = json!({
        "iss": ScriptedHttp::origin(), "sub": "jwt-user", "aud": "client",
        "scope": "read", "exp": OpenidConnect::now_unix_secs() + 3600
    });
    let sign = |claims: &serde_json::Value, algorithm, secret: &[u8]| {
        let mut header = Header::new(algorithm);
        header.kid = Some("policy-key".to_string());
        jsonwebtoken::encode(&header, claims, &jsonwebtoken::EncodingKey::from_secret(secret)).unwrap()
    };
    let secret = b"signing-policy-test-secret";
    let mut cases = vec![
        (sign(&claims, Algorithm::HS256, secret), Ok("jwt-user")),
        ("opaque-token".to_string(), Ok("introspection-user")),
        (sign(&json!([]), Algorithm::HS256, secret), Ok("introspection-user")),
        (
            sign(&claims, Algorithm::HS384, secret),
            Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm)),
        ),
        (
            sign(&claims, Algorithm::HS256, b"wrong-signing-secret"),
            Err(OidcVerifyError::Auth(AuthFailure::InvalidSignature)),
        ),
    ];
    for (field, value, error) in [
        (
            "iss",
            json!("https://wrong.example"),
            OidcVerifyError::Auth(AuthFailure::InvalidIssuer),
        ),
        (
            "aud",
            json!("wrong-audience"),
            OidcVerifyError::Auth(AuthFailure::InvalidAudience),
        ),
        ("exp", json!(1), OidcVerifyError::Expired),
        (
            "scope",
            json!("write"),
            OidcVerifyError::Auth(AuthFailure::InsufficientScope),
        ),
    ] {
        let mut invalid = claims.clone();
        invalid[field] = value;
        cases.push((sign(&invalid, Algorithm::HS256, secret), Err(error)));
    }
    for use_jwks in [false, true] {
        for (token, expected) in &cases {
            let mut config = base_config();
            config.verification_mode = VerificationMode::Auto;
            config.use_jwks = use_jwks;
            config.token_signing_alg = Some("HS256".to_string());
            config.allowed_signing_algs = Some(vec!["HS256".to_string()]);
            config.audiences = Some(vec!["client".to_string()]);
            config.required_scopes = Some(vec!["read".to_string()]);
            config.clock_skew_seconds = 0;
            config.introspection_cache_ttl = 60;
            let plugin = OpenidConnect::new(&config, "default".to_string());
            let script = signing_policy_script();
            plugin.jwks_provider.install_scripted_http(script.clone());
            plugin
                .jwks_provider
                .seed_discovery(DiscoveryDocument {
                    issuer: ScriptedHttp::origin().to_string(),
                    jwks_uri: ScriptedHttp::jwks_url(),
                    authorization_endpoint: None,
                    token_endpoint: None,
                    introspection_endpoint: Some("http://oidc.test/introspect".to_string()),
                    userinfo_endpoint: None,
                    end_session_endpoint: None,
                    revocation_endpoint: None,
                })
                .await;
            let mut cached = claims.clone();
            cached["sub"] = json!("introspection-user");
            cached["active"] = json!(true);
            plugin.cache_introspection_claims(token, &Arc::new(cached)).await;
            let result = plugin
                .verify_token(&mut MockPluginSession::new(), token)
                .await
                .map(|value| value["sub"].as_str().unwrap().to_string());
            assert_eq!(
                result,
                if use_jwks {
                    expected.map(str::to_owned)
                } else {
                    Ok("introspection-user".to_string())
                }
            );
            if !use_jwks
                || token == "opaque-token"
                || *expected == Err(OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm))
            {
                assert_eq!(script.hits(JWKS_PATH), 0);
            } else {
                assert_eq!(script.hits(JWKS_PATH), 1);
            }
        }
    }
}

#[tokio::test]
async fn jwt_exp_is_required_even_when_expiration_comparison_is_disabled() {
    use super::super::error::{AuthFailure, OidcVerifyError};
    use super::super::jwks_provider::ScriptedHttp;

    let mut config = base_config();
    config.token_signing_alg = Some("HS256".to_string());
    config.allowed_signing_algs = Some(vec!["HS256".to_string()]);
    config.clock_skew_seconds = 0;
    let plugin = OpenidConnect::new(&config, "default".to_string());
    plugin.jwks_provider.install_scripted_http(signing_policy_script());
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("policy-key".to_string());
    let future = OpenidConnect::now_unix_secs() + 3600;
    for (exp, validate_exp, expected) in [
        (None, true, Err(OidcVerifyError::Auth(AuthFailure::InvalidToken))),
        (None, false, Err(OidcVerifyError::Auth(AuthFailure::InvalidToken))),
        (Some(future), true, Ok(())),
        (Some(future), false, Ok(())),
        (Some(1), true, Err(OidcVerifyError::Expired)),
        (Some(1), false, Ok(())),
    ] {
        // Leave nbf, aud and sub absent: they are not required claims.
        let mut claims = json!({"iss": ScriptedHttp::origin()});
        if let Some(exp) = exp {
            claims["exp"] = json!(exp);
        }
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(b"signing-policy-test-secret"),
        )
        .unwrap();
        let mut session = MockPluginSession::new();
        assert_eq!(
            plugin
                .verify_jwt_token_internal(&mut session, &token, true, None, validate_exp)
                .await
                .map(|_| ()),
            expected,
            "exp={exp:?}, validate_exp={validate_exp}"
        );
    }
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
        let claims = Arc::new(json!({
            "active": true, "iss": "https://issuer.example.com",
            "exp": OpenidConnect::now_unix_secs() + 120
        }));
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

#[tokio::test(start_paused = true)]
async fn every_upstream_continuation_propagates_cookie_cleanup_failure() {
    for scenario in ["bearer", "session", "refresh", "stale", "pass"] {
        for (error, status) in [
            (crate::request_cookie::CookieEditError::Unsanitizable, 400),
            (crate::request_cookie::CookieEditError::Internal, 500),
        ] {
            let mut cfg = base_config();
            cfg.bearer_only = scenario == "bearer";
            cfg.unauth_action = UnauthAction::Pass;
            cfg.verification_mode = VerificationMode::IntrospectionOnly;
            cfg.introspection_cache_ttl = 60;
            cfg.renew_access_token_on_expiry = true;
            cfg.clock_skew_seconds = 0;
            cfg.access_token_expires_leeway = if scenario == "stale" { 300 } else { 0 };
            cfg.timeout = 1;
            cfg.token_signing_alg = Some("HS256".into());
            cfg.allowed_signing_algs = Some(vec!["HS256".into()]);
            cfg.resolved_session_secret = Some("dummy-session-secret-32-bytes-long!!".into());
            cfg.resolved_oidc_client_secret = Some("test-client-secret".into());
            let plugin = OpenidConnect::new(&cfg, "default".into());
            plugin
                .jwks_provider
                .seed_discovery(DiscoveryDocument {
                    issuer: "https://idp.example.com".into(),
                    jwks_uri: "http://169.254.169.254/jwks".into(),
                    authorization_endpoint: None,
                    token_endpoint: None,
                    introspection_endpoint: Some("http://169.254.169.254/introspect".into()),
                    userinfo_endpoint: None,
                    end_session_endpoint: None,
                    revocation_endpoint: None,
                })
                .await;
            let now = OpenidConnect::now_unix_secs();
            let exp = if matches!(scenario, "refresh" | "stale") {
                now - 120
            } else {
                now + 3600
            };
            let claims = json!({"active":true, "iss":"https://idp.example.com", "exp":exp});
            let token = jsonwebtoken::encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &jsonwebtoken::EncodingKey::from_secret(b"secret"),
            )
            .unwrap();
            plugin.introspection_cache.insert(
                OpenidConnect::introspection_cache_key(&token),
                IntrospectionCacheEntry {
                    claims: Arc::new(claims),
                },
                Duration::from_secs(60),
            );
            let payload = OidcSessionCookie {
                access_token: token.clone(),
                created_at: now,
                session_expires_at: now + 3600,
                expires_at: Some(now + 3600),
                id_token: None,
                refresh_token: Some("refresh-token".into()),
            };
            let encoded =
                OpenidConnect::encode_signed_cookie_payload(&payload, plugin.session_cipher().unwrap()).unwrap();
            let lock_key = OpenidConnect::refresh_lock_key(&encoded);
            if scenario == "refresh" {
                plugin
                    .cache_introspection_claims(
                        "replacement-token",
                        &Arc::new(json!({"active":true,"iss":"https://idp.example.com","exp":now+3600})),
                    )
                    .await;
                let mut refreshed = payload.clone();
                refreshed.access_token = "replacement-token".into();
                plugin.put_refresh_result(&lock_key, refreshed).await;
            }
            let flight = if scenario == "stale" {
                plugin
                    .jwks_provider
                    .seed_stale_jwks(
                        serde_json::from_value(json!({"keys":[{"kty":"oct","k":"c2VjcmV0","alg":"HS256"}]})).unwrap(),
                    )
                    .await;
                Some(plugin.refresh_singleflight.get_or_create(&lock_key))
            } else {
                None
            };
            let held_guard = match &flight {
                Some(flight) => Some(flight.lock().lock().await),
                None => None,
            };
            let mut session = MockPluginSession::new();
            session.expect_get_path().return_const("/".to_owned());
            session.expect_get_query_param().returning(|_| None);
            let mut headers = http::HeaderMap::new();
            if scenario == "bearer" {
                headers.insert(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
                );
            }
            if !matches!(scenario, "bearer" | "pass") {
                headers.insert(
                    http::header::COOKIE,
                    http::HeaderValue::from_str(&format!("edgion_oidc_session={encoded}")).unwrap(),
                );
            }
            session.expect_request_header_map().return_const(headers);
            session.expect_set_request_header_value().returning(|_, _| Ok(()));
            session.expect_remove_request_header().returning(|_| Ok(()));
            session.expect_append_response_header().returning(|_, _| Ok(()));
            session.expect_record_outcome().returning(|_, _| {});
            session
                .expect_remove_request_cookies()
                .withf(|names| names == gateway_owned_cookie_names().as_slice())
                .times(1)
                .returning(move |_| Err(error));
            session.expect_write_response_header().never();
            session.expect_write_response_body().never();
            let mut log = PluginLog::new("OpenidConnect");
            let result = plugin.run_request(&mut session, &mut log).await;
            drop(held_guard);
            assert_eq!(
                result,
                PluginRunningResult::ErrResponse { status, body: None },
                "{scenario}"
            );
            let entries = plugin_log_entries(&log);
            let expected = match scenario {
                "session" => Some("session token verified"),
                "refresh" => Some("token refresh on expiry"),
                "stale" => Some("token refresh wait timeout; stale token within leeway"),
                "pass" => Some("no token; pass"),
                _ => None,
            };
            if let Some(expected) = expected {
                assert!(entries.iter().any(|entry| entry == expected), "{scenario}");
            }
        }
    }
}

#[tokio::test]
async fn prepared_oidc_challenge_preserves_dynamic_descriptions_and_utf8_realm() {
    let mut config = base_config();
    config.realm = "café \"API\"\\".to_string();
    let oidc = actual_oidc(&config, "default".to_string()).unwrap();
    for body in ["missing scope", "denied \"scope\"\\\r\n"] {
        let escaped = body
            .replace(['\r', '\n'], "")
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let expected = format!(
            "Bearer realm=\"café \\\"API\\\"\\\\\", error=\"insufficient_scope\", error_description=\"{escaped}\""
        );
        let mut session = MockPluginSession::new();
        session
            .expect_write_response_header()
            .times(1)
            .returning(move |header, _| {
                assert_eq!(header.status.as_u16(), 403);
                assert_eq!(header.headers["www-authenticate"].as_bytes(), expected.as_bytes());
                Ok(())
            });
        session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
        session.expect_shutdown().times(1).returning(|| {});
        oidc.send_forbidden(&mut session, body).await.unwrap();
    }
}

#[tokio::test]
async fn allow_expired_keeps_nbf_and_claim_format_checks() {
    use super::super::error::{AuthFailure, OidcVerifyError};
    use super::super::jwks_provider::ScriptedHttp;
    let mut config = base_config();
    config.token_signing_alg = Some("HS256".into());
    config.allowed_signing_algs = Some(vec!["HS256".into()]);
    config.clock_skew_seconds = 0;
    let plugin = OpenidConnect::new(&config, "default".into());
    plugin.jwks_provider.install_scripted_http(signing_policy_script());
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("policy-key".into());
    let future = OpenidConnect::now_unix_secs() + 3600;
    for validate_exp in [true, false] {
        for (nbf, expected) in [
            (None, Ok(())),
            (Some(json!(0)), Ok(())),
            (
                Some(json!(future)),
                Err(OidcVerifyError::Auth(AuthFailure::NotYetValid)),
            ),
            (Some(json!("not-a-date")), Err(OidcVerifyError::InvalidFormat)),
        ] {
            let mut claims = json!({"iss":ScriptedHttp::origin(),"exp":if validate_exp {future} else {1}});
            if let Some(nbf) = nbf {
                claims["nbf"] = nbf;
            }
            let token = jsonwebtoken::encode(
                &header,
                &claims,
                &jsonwebtoken::EncodingKey::from_secret(b"signing-policy-test-secret"),
            )
            .unwrap();
            assert_eq!(
                plugin
                    .verify_jwt_token_internal(&mut MockPluginSession::new(), &token, true, None, validate_exp)
                    .await
                    .map(|_| ()),
                expected
            );
        }
    }
}

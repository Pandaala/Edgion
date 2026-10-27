use std::collections::HashMap;

use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtAlgorithm, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};

use super::*;

fn assert_send_sync<T: Send + Sync>() {}

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

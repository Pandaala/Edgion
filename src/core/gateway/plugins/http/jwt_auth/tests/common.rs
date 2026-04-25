//! Shared helpers for jwt_auth tests.

use base64::Engine;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) use super::super::JwtAuth;
pub(super) use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
pub(super) use crate::core::gateway::plugins::runtime::{PluginLog, RequestFilter};
pub(super) use crate::types::filters::PluginRunningResult;
use crate::types::plugin_configs::ResolvedJwtCredential;
pub(super) use crate::types::resources::edgion_plugins::{JwtAlgorithm, JwtAuthConfig};

pub(super) const TEST_SECRET: &str = "test-secret-key-for-jwt-auth-testing-min-32-bytes";

/// Helper: create JwtAuth with default config
pub(super) fn jwt_auth_default(anonymous: Option<String>) -> JwtAuth {
    let config = JwtAuthConfig {
        anonymous,
        ..Default::default()
    };
    JwtAuth::new(&config, "default".to_string())
}

/// Helper: create JwtAuth with resolved credential
/// Note: secret is automatically base64-encoded as expected by the runtime
pub(super) fn jwt_auth_with_credential(
    secret: &str,
    algorithm: JwtAlgorithm,
    config_fn: Option<fn(&mut JwtAuthConfig)>,
) -> JwtAuth {
    let mut config = JwtAuthConfig {
        algorithm,
        ..Default::default()
    };
    // Controller base64-encodes the secret, so we do the same in tests
    let secret_b64 = base64::engine::general_purpose::STANDARD.encode(secret.as_bytes());
    config.resolved_credential = Some(ResolvedJwtCredential {
        key: None,
        secret: Some(secret_b64),
        public_key: None,
    });
    if let Some(f) = config_fn {
        f(&mut config);
    }
    JwtAuth::new(&config, "default".to_string())
}

/// Helper: generate HS256 JWT token
pub(super) fn generate_hs256_token(claims: serde_json::Value, secret: &str) -> String {
    let header = Header::new(Algorithm::HS256);
    encode(&header, &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
}

/// Helper: generate JWT with specific algorithm
pub(super) fn generate_token_with_alg(claims: serde_json::Value, secret: &str, alg: Algorithm) -> String {
    let header = Header::new(alg);
    encode(&header, &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
}

/// Helper: get current timestamp
pub(super) fn now_ts() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// Assert that no base64url segment of the token appears verbatim in serialized log.
pub(super) fn assert_no_token_leak(log: &PluginLog, token: &str) {
    let serialized = serde_json::to_string(log).expect("PluginLog serialize");
    for segment in token.split('.').filter(|s| s.len() >= 6) {
        assert!(
            !serialized.contains(segment),
            "PluginLog leaked token segment `{}` in `{}`",
            segment,
            serialized
        );
    }
}

pub(super) async fn run_and_assert_reject(auth: JwtAuth, token: &str) -> PluginLog {
    let mut mock = MockPluginSession::new();
    let mut log = PluginLog::new("JwtAuth");
    mock.expect_method().returning(|| "GET".to_string());
    let token_clone = token.to_string();
    mock.expect_header_value()
        .returning(move |_| Some(format!("Bearer {}", token_clone)));
    mock.expect_get_query_param().returning(|_| None);
    mock.expect_get_cookie().returning(|_| None);
    mock.expect_write_response_header().returning(|_, _| Ok(()));
    mock.expect_write_response_body().returning(|_, _| Ok(()));
    mock.expect_shutdown().returning(|| {});

    let r = auth.run_request(&mut mock, &mut log).await;
    assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
    log
}

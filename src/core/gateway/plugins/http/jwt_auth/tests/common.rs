//! Shared helpers for jwt_auth tests.

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) use super::super::JwtAuth;
pub(super) use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
pub(super) use crate::core::gateway::plugins::runtime::{PluginLog, RequestFilter};
pub(super) use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::plugin_configs::{ResolvedJwtCredential, ResolvedJwtIssuerGroup};
pub(super) use crate::types::resources::edgion_plugins::{JwtAlgorithm, JwtAuthConfig};

/// Default secret used across most HS256 tests.
pub(super) const TEST_SECRET: &str = "test-secret-key-for-jwt-auth-testing-min-32-bytes";
/// Default kid (key identifier) used across most tests.
pub(super) const TEST_KID: &str = "test-kid-1";
/// Default issuer used across most tests.
pub(super) const TEST_ISS: &str = "https://test.example.com";

/// Helper: create JwtAuth with no credentials (anonymous-capable or no-credential).
pub(super) fn jwt_auth_default(anonymous: Option<String>) -> JwtAuth {
    let config = JwtAuthConfig {
        anonymous,
        ..Default::default()
    };
    JwtAuth::new(&config, "default".to_string())
}

/// Helper: create JwtAuth backed by a single HS* credential.
///
/// Builds a store with one issuer group (`TEST_ISS` → `TEST_KID` → `secret`).
/// Tokens must include `"iss": TEST_ISS` and `"key": TEST_KID` claims.
/// The `config_fn` can adjust other fields (base64_secret, realm, etc.).
pub(super) fn jwt_auth_with_credential(
    secret: &str,
    algorithm: JwtAlgorithm,
    config_fn: Option<fn(&mut JwtAuthConfig)>,
) -> JwtAuth {
    let mut config = JwtAuthConfig {
        algorithm,
        ..Default::default()
    };
    if let Some(f) = config_fn {
        f(&mut config);
    }

    // Build resolved_groups_by_iss for the single test group.
    let mut creds = HashMap::new();
    creds.insert(
        TEST_KID.to_string(),
        ResolvedJwtCredential {
            key: Some(TEST_KID.to_string()),
            secret: Some(secret.to_string()),
            public_key: None,
        },
    );
    let group = ResolvedJwtIssuerGroup {
        group: "test-group".to_string(),
        algorithm,
        audiences: config.audiences.clone(),
        credentials: creds,
    };
    let mut by_iss = HashMap::new();
    by_iss.insert(TEST_ISS.to_string(), group);
    config.resolved_groups_by_iss = Some(by_iss);

    JwtAuth::new(&config, "default".to_string())
}

/// Helper: generate HS256 JWT token with TEST_ISS and TEST_KID.
pub(super) fn generate_hs256_token(claims: serde_json::Value, secret: &str) -> String {
    // Merge iss and key into claims if not already present.
    let mut merged = claims.as_object().cloned().unwrap_or_default();
    merged
        .entry("iss")
        .or_insert(serde_json::Value::String(TEST_ISS.to_string()));
    merged
        .entry("key")
        .or_insert(serde_json::Value::String(TEST_KID.to_string()));
    let merged_value = serde_json::Value::Object(merged);
    let header = Header::new(Algorithm::HS256);
    encode(&header, &merged_value, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
}

/// Helper: generate JWT with specific algorithm, TEST_ISS, and TEST_KID.
pub(super) fn generate_token_with_alg(claims: serde_json::Value, secret: &str, alg: Algorithm) -> String {
    let mut merged = claims.as_object().cloned().unwrap_or_default();
    merged
        .entry("iss")
        .or_insert(serde_json::Value::String(TEST_ISS.to_string()));
    merged
        .entry("key")
        .or_insert(serde_json::Value::String(TEST_KID.to_string()));
    let merged_value = serde_json::Value::Object(merged);
    let header = Header::new(alg);
    encode(&header, &merged_value, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
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

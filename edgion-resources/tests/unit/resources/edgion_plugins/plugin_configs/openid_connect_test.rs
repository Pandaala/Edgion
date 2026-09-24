use super::{OpenidConnectConfig, TinyUfoTtlCacheConfig, UnauthAction, VerificationMode};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn oidc_cache_defaults_inherit_gateway_limits() {
    let cfg = OpenidConnectConfig::default();
    assert_eq!(cfg.introspection.tiny_ufo_ttl_cache.max_entries, 0);
    assert_eq!(cfg.access_token.lru_cache.capacity, 0);
    assert_eq!(cfg.introspection.tiny_ufo_ttl_cache.max_logical_bytes, None);
    assert_eq!(cfg.access_token.lru_cache.shards, 0);
}

#[test]
fn oidc_cache_schema_recognizes_algorithm_specific_paths() {
    let schema = schemars::schema_for!(OpenidConnectConfig);
    let value = json!({
        "discovery": "https://idp.example.com/.well-known/openid-configuration",
        "clientId": "client",
        "introspection": {
            "tinyUfoTtlCache": {
                "maxEntries": 1024,
                "maxLogicalBytes": "4MiB"
            }
        },
        "accessToken": {
            "lruCache": {
                "capacity": 2048,
                "shards": 8
            }
        }
    });
    assert!(crate::schema_check::unknown_key_paths(&value, &schema).is_empty());
}

#[test]
fn oidc_cache_schema_rejects_wrong_cache_kinds() {
    let schema = schemars::schema_for!(OpenidConnectConfig);
    let value = json!({
        "discovery": "https://idp.example.com/.well-known/openid-configuration",
        "clientId": "client",
        "introspection": {
            "lruCache": {"capacity": 1024, "shards": 2},
            "memoryCache": {"maxEntries": 1024}
        },
        "accessToken": {
            "memoryCache": {"maxEntries": 2048},
            "tinyUfoTtlCache": {"maxEntries": 2048}
        }
    });
    assert_eq!(
        crate::schema_check::unknown_key_paths(&value, &schema),
        vec![
            "accessToken.memoryCache".to_string(),
            "accessToken.tinyUfoTtlCache".to_string(),
            "introspection.lruCache".to_string(),
            "introspection.memoryCache".to_string(),
        ]
    );
}

#[test]
fn oidc_rejects_invalid_logical_record_budget_during_validation() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        introspection: super::OidcIntrospectionCacheSection {
            tiny_ufo_ttl_cache: TinyUfoTtlCacheConfig {
                max_logical_bytes: Some("invalid".to_string()),
                ..Default::default()
            },
        },
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("maxLogicalBytes must be a valid byte size")
    );
}

#[test]
fn oidc_rejects_unsafe_access_token_lru_limits() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "client".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        access_token: super::OidcAccessTokenCacheSection {
            lru_cache: super::LruCacheConfig { capacity: 4, shards: 5 },
        },
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("lruCache.shards must not exceed capacity")
    );
}

#[test]
fn test_validation_requires_discovery_and_client_id() {
    let cfg = OpenidConnectConfig::default();
    assert_eq!(cfg.get_validation_error(), Some("discovery is required"));
}

#[test]
fn test_validation_bearer_only_jwks_allows_no_client_secret() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        ..Default::default()
    };
    assert!(cfg.get_validation_error().is_none());
}

#[test]
fn test_validation_non_bearer_requires_client_secret() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-web-app".to_string(),
        bearer_only: false,
        unauth_action: UnauthAction::Auth,
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("clientSecretRef is required for non-bearer-only/introspection flows")
    );
}

#[test]
fn test_validation_reject_none_algorithm() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        token_signing_alg: Some("none".to_string()),
        ..Default::default()
    };
    assert_eq!(cfg.get_validation_error(), Some("tokenSigningAlg cannot be 'none'"));
}

#[test]
fn test_validation_jwks_only_requires_use_jwks() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        use_jwks: false,
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("useJwks must be true when verificationMode is JwksOnly")
    );
}

#[test]
fn test_validation_reject_invalid_redirect_uri() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        redirect_uri: Some("oidc/callback".to_string()),
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("redirectUri must be absolute URL or absolute path")
    );
}

#[test]
fn test_validation_auth_flow_requires_session_secret() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-web-app".to_string(),
        bearer_only: false,
        unauth_action: UnauthAction::Auth,
        resolved_oidc_client_secret: Some("client-secret".to_string()),
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("sessionSecretRef is required when unauthAction=Auth")
    );
}

#[test]
fn test_validation_reject_invalid_logout_path() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        logout_path: "logout".to_string(),
        ..Default::default()
    };
    assert_eq!(cfg.get_validation_error(), Some("logoutPath must start with '/'"));
}

#[test]
fn test_validation_reject_zero_max_session_cookie_bytes() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-web-app".to_string(),
        bearer_only: false,
        unauth_action: UnauthAction::Auth,
        resolved_oidc_client_secret: Some("client-secret".to_string()),
        resolved_session_secret: Some("session-secret".to_string()),
        max_session_cookie_bytes: 0,
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("maxSessionCookieBytes must be greater than 0")
    );
}

#[test]
fn test_validation_reject_invalid_session_cookie_same_site() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-web-app".to_string(),
        bearer_only: false,
        unauth_action: UnauthAction::Auth,
        resolved_oidc_client_secret: Some("client-secret".to_string()),
        resolved_session_secret: Some("session-secret".to_string()),
        session_cookie_same_site: "Invalid".to_string(),
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("sessionCookieSameSite must be one of: Strict, Lax, None")
    );
}

#[test]
fn test_validation_none_same_site_requires_secure_cookie() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-web-app".to_string(),
        bearer_only: false,
        unauth_action: UnauthAction::Auth,
        resolved_oidc_client_secret: Some("client-secret".to_string()),
        resolved_session_secret: Some("session-secret".to_string()),
        session_cookie_same_site: "None".to_string(),
        session_cookie_secure: false,
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("sessionCookieSecure must be true when sessionCookieSameSite is None")
    );
}

#[test]
fn test_validation_reject_reserved_authorization_params() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-web-app".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        authorization_params: Some(HashMap::from([("response_type".to_string(), "token".to_string())])),
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("authorizationParams cannot contain reserved OIDC keys")
    );
}

#[test]
fn test_validation_revoke_on_logout_requires_client_secret() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        revoke_tokens_on_logout: true,
        ..Default::default()
    };
    assert_eq!(
        cfg.get_validation_error(),
        Some("clientSecretRef is required for non-bearer-only/introspection flows")
    );
}

#[test]
fn openid_drops_ssl_verify_accepts_verify() {
    let c: OpenidConnectConfig = serde_json::from_value(serde_json::json!({
        "clientId":"x","clientSecret":"y","discovery":"https://i/.well-known/openid-configuration",
        "tls": { "verify": false }
    }))
    .unwrap();
    assert_eq!(c.tls.verify, Some(false));
}

#[test]
fn validation_rejects_case_insensitive_claim_header_destination_collision() {
    let cfg = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        verification_mode: VerificationMode::JwksOnly,
        claims_to_headers: Some(HashMap::from([
            ("sub".to_string(), "X-Identity".to_string()),
            ("email".to_string(), "x-identity".to_string()),
        ])),
        ..Default::default()
    };

    assert_eq!(
        cfg.get_validation_error(),
        Some(super::super::DUPLICATE_HEADER_DESTINATION)
    );
}

#[test]
fn validation_rejects_claim_destination_owned_by_enabled_fixed_header() {
    for destination in ["x-access-token", "x-id-token", "x-userinfo", "authorization"] {
        let mut cfg = OpenidConnectConfig {
            discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
            client_id: "my-api".to_string(),
            bearer_only: true,
            verification_mode: VerificationMode::JwksOnly,
            claims_to_headers: Some(HashMap::from([("sub".to_string(), destination.to_string())])),
            ..Default::default()
        };
        match destination {
            "x-access-token" => cfg.set_access_token_header = true,
            "x-id-token" => cfg.set_id_token_header = true,
            "x-userinfo" => cfg.set_userinfo_header = true,
            "authorization" => cfg.access_token_in_authorization_header = true,
            _ => unreachable!(),
        }

        assert_eq!(
            cfg.get_validation_error(),
            Some(super::super::DUPLICATE_HEADER_DESTINATION),
            "destination: {destination}"
        );
    }
}

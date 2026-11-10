use super::{OpenidConnectConfig, TinyUfoTtlCacheConfig, UnauthAction, VerificationMode};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn oidc_cache_defaults_inherit_gateway_limits() {
    let cfg = OpenidConnectConfig::default();
    assert_eq!(cfg.introspection.tiny_ufo_ttl_cache.max_entries, 0);
    assert_eq!(cfg.introspection.tiny_ufo_ttl_cache.max_logical_bytes, None);
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
        "maxSessionCookieBytes": 3800,
        "maxSessionCookieChunks": 8
    });
    assert!(crate::schema_check::unknown_key_paths(&value, &schema).is_empty());
}

#[test]
fn oidc_access_token_cache_section_is_an_unknown_key() {
    // The session cookie owns the access token; there is no access-token cache to size.
    let schema = schemars::schema_for!(OpenidConnectConfig);
    let value = json!({
        "discovery": "https://idp.example.com/.well-known/openid-configuration",
        "clientId": "client",
        "accessToken": {"lruCache": {"capacity": 2048, "shards": 8}}
    });
    assert_eq!(
        crate::schema_check::unknown_key_paths(&value, &schema),
        vec!["accessToken".to_string()]
    );
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
        }
    });
    assert_eq!(
        crate::schema_check::unknown_key_paths(&value, &schema),
        vec![
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
fn test_validation_bounds_jwks_intervals() {
    // auth-03: unbounded values overflowed the Gateway's `Instant + Duration` JWKS deadline.
    let base = OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-api".to_string(),
        bearer_only: true,
        ..Default::default()
    };
    let max = super::MAX_JWKS_INTERVAL_SECS;
    let ttl_error = Some("jwksCacheTtl must be between 1 and 31536000 seconds");
    let refresh_error = Some("jwksMinRefreshInterval must be between 1 and 31536000 seconds");

    for (ttl, refresh, expected) in [
        (max, max, None),
        (1, 1, None),
        (0, 10, ttl_error),
        (max + 1, 10, ttl_error),
        (u64::MAX, 10, ttl_error),
        (300, 0, refresh_error),
        (300, max + 1, refresh_error),
        (300, u64::MAX, refresh_error),
    ] {
        let cfg = OpenidConnectConfig {
            jwks_cache_ttl: ttl,
            jwks_min_refresh_interval: refresh,
            ..base.clone()
        };
        assert_eq!(cfg.get_validation_error(), expected, "ttl={ttl} refresh={refresh}");
    }
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
fn validation_rejects_protocol_owned_host_claim_destination() {
    for destination in ["Host", "host", "HOST", "hOsT", "X-Tenant"] {
        let config = OpenidConnectConfig {
            discovery: "https://idp.example.com/discovery".to_string(),
            client_id: "client".to_string(),
            bearer_only: true,
            verification_mode: VerificationMode::JwksOnly,
            claims_to_headers: Some(HashMap::from([("tenant".to_string(), destination.to_string())])),
            ..Default::default()
        };
        let expected = (destination != "X-Tenant")
            .then_some(super::super::auth_header_projection::PROTOCOL_OWNED_CLAIM_DESTINATION);
        assert_eq!(config.get_validation_error(), expected);
        assert_eq!(config.compile_claim_header_projection().err(), expected);
    }
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

fn session_cookie_limit_config(bytes: u64, chunks: u32) -> OpenidConnectConfig {
    OpenidConnectConfig {
        discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
        client_id: "my-web-app".to_string(),
        bearer_only: false,
        unauth_action: UnauthAction::Auth,
        resolved_oidc_client_secret: Some("client-secret".to_string()),
        resolved_session_secret: Some("session-secret".to_string()),
        max_session_cookie_bytes: bytes,
        max_session_cookie_chunks: chunks,
        ..Default::default()
    }
}

#[test]
fn session_cookie_chunk_defaults_fit_the_total_budget() {
    let cfg = OpenidConnectConfig::default();
    assert_eq!(cfg.max_session_cookie_bytes, 3800);
    assert_eq!(cfg.max_session_cookie_chunks, 8);
    let parsed: OpenidConnectConfig = serde_json::from_value(json!({
        "discovery": "https://idp.example.com/.well-known/openid-configuration",
        "clientId": "client"
    }))
    .unwrap();
    assert_eq!(parsed.max_session_cookie_chunks, 8);
    assert_eq!(session_cookie_limit_config(3800, 8).get_validation_error(), None);
}

#[test]
fn session_cookie_chunk_count_range_is_enforced() {
    let message = Some("maxSessionCookieChunks must be between 1 and 16");
    assert_eq!(session_cookie_limit_config(1024, 0).get_validation_error(), message);
    assert_eq!(session_cookie_limit_config(1024, 17).get_validation_error(), message);
    assert_eq!(session_cookie_limit_config(1024, 1).get_validation_error(), None);
    assert_eq!(session_cookie_limit_config(2048, 16).get_validation_error(), None);
}

#[test]
fn session_cookie_total_budget_is_enforced_without_overflow() {
    let message = Some("maxSessionCookieBytes * maxSessionCookieChunks must not exceed 32768");
    assert_eq!(session_cookie_limit_config(4096, 8).get_validation_error(), None);
    assert_eq!(session_cookie_limit_config(4097, 8).get_validation_error(), message);
    assert_eq!(session_cookie_limit_config(8192, 8).get_validation_error(), message);
    // A wrapping multiply would yield 0 and pass; the checked multiply must reject it.
    assert_eq!(
        session_cookie_limit_config(1u64 << 61, 8).get_validation_error(),
        message
    );
    assert_eq!(
        session_cookie_limit_config(u64::MAX, 16).get_validation_error(),
        message
    );
}

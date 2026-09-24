use super::*;

use base64::engine::general_purpose::STANDARD as BASE64;

use crate::link_sys::credential::{CredentialState, InvalidReason, ResolvedCredential};
use edgion_resources::credentials::materials::{BasicAuthMaterial, BearerMaterial, EsApiKeyMaterial};

/// Shorthand: `NotRequired` state.
fn no_auth() -> CredentialState<ResolvedCredential> {
    CredentialState::NotRequired
}

fn basic_auth_crd() -> ElasticsearchClientConfig {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    ElasticsearchClientConfig {
        auth: Some(ElasticsearchAuth::Basic {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "es-creds".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        ..Default::default()
    }
}

#[test]
fn test_build_es_client_simple() {
    let crd = ElasticsearchClientConfig::default();
    let (_, headers) = build_es_client(&crd, &no_auth()).unwrap();
    assert!(headers.get("content-type").is_some());
    assert!(headers.get("authorization").is_none());
}

#[test]
fn test_build_es_client_basic_auth() {
    let crd = basic_auth_crd();
    let state = CredentialState::Resolved(ResolvedCredential::Basic(BasicAuthMaterial {
        username: Some("user".to_string()),
        password: "secret".to_string(),
    }));
    let (_, headers) = build_es_client(&crd, &state).unwrap();
    let auth = headers.get("authorization").unwrap().to_str().unwrap();
    let expected = format!("Basic {}", BASE64.encode("user:secret"));
    assert_eq!(auth, expected);
    assert!(headers[AUTHORIZATION].is_sensitive());
    assert!(!format!("{headers:?}").contains(&expected));
}

#[test]
fn test_build_es_client_api_key_auth() {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    let crd = ElasticsearchClientConfig {
        auth: Some(ElasticsearchAuth::ApiKey {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "es-api-key".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        ..Default::default()
    };
    let state = CredentialState::Resolved(ResolvedCredential::EsApiKey(EsApiKeyMaterial {
        api_key_id: "id".to_string(),
        api_key_secret: "sec".to_string(),
    }));
    let (_, headers) = build_es_client(&crd, &state).unwrap();
    let auth = headers.get("authorization").unwrap().to_str().unwrap();
    let expected = format!("ApiKey {}", BASE64.encode("id:sec"));
    assert_eq!(auth, expected);
    assert!(headers[AUTHORIZATION].is_sensitive());
    assert!(!format!("{headers:#?}").contains(&expected));
}

/// `Pending` state → fail-fast.
#[test]
fn test_build_es_client_auth_pending_fails_fast() {
    let crd = basic_auth_crd();
    let err = build_es_client(&crd, &CredentialState::Pending).expect_err("should refuse pending");
    assert!(
        err.to_string().contains("has not been resolved"),
        "unexpected error message: {err}"
    );
}

/// `Invalid` state → fail-fast with reason surfaced.
#[test]
fn test_build_es_client_auth_invalid_fails_fast() {
    let crd = basic_auth_crd();
    let state = CredentialState::Invalid(InvalidReason::ParseFailed("missing keys: [password]".into()));
    let err = build_es_client(&crd, &state).expect_err("should refuse invalid");
    assert!(
        err.to_string().contains("invalid") && err.to_string().contains("missing"),
        "unexpected error message: {err}"
    );
}

/// Invariant guard: CRD has `auth` but state says `NotRequired` — this
/// can only happen if `credential::resolved_state` is buggy. Fail-fast.
#[test]
fn test_build_es_client_crd_has_auth_but_state_not_required_is_internal_bug() {
    let crd = basic_auth_crd();
    let err = build_es_client(&crd, &CredentialState::NotRequired).expect_err("should refuse");
    assert!(
        err.to_string().contains("internal bug"),
        "unexpected error message: {err}"
    );
}

#[test]
fn test_build_es_client_bearer_auth() {
    use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
    let crd = ElasticsearchClientConfig {
        auth: Some(ElasticsearchAuth::Bearer {
            slot: SecretSlot {
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "es-token".to_string(),
                    namespace: None,
                },
                secret: None,
            },
        }),
        ..Default::default()
    };
    let state = CredentialState::Resolved(ResolvedCredential::Bearer(BearerMaterial {
        token: "tokval".to_string(),
    }));
    let (_, headers) = build_es_client(&crd, &state).unwrap();
    let auth = headers.get("authorization").unwrap().to_str().unwrap();
    assert_eq!(auth, "Bearer tokval");
    assert!(headers[AUTHORIZATION].is_sensitive());
    assert!(!format!("{headers:?}").contains("tokval"));
}

#[test]
fn test_bulk_config_defaults() {
    let crd = ElasticsearchClientConfig::default();
    let config = EsBulkConfig::from_crd(&crd);
    assert_eq!(config.batch_size, DEFAULT_BATCH_SIZE);
    assert_eq!(config.flush_interval, Duration::from_millis(DEFAULT_FLUSH_INTERVAL_MS));
    assert_eq!(config.max_retries, DEFAULT_BULK_RETRIES);
    assert_eq!(config.index_prefix, DEFAULT_INDEX_PREFIX);
    assert_eq!(config.date_pattern, DEFAULT_DATE_PATTERN);
}

#[test]
fn test_bulk_config_clamped_to_max() {
    let crd = ElasticsearchClientConfig {
        bulk: Some(EsBulk {
            batch_size: Some(100_000), // Over max
            flush_interval: Some(MAX_FLUSH_INTERVAL_MS + 1),
            max_retries: Some(100), // Over max
            backoff_ms: None,
            max_body_bytes: None,
            channel_size: Some(1_000_000), // Over max
        }),
        ..Default::default()
    };
    let config = EsBulkConfig::from_crd(&crd);
    assert_eq!(config.batch_size, MAX_BATCH_SIZE);
    assert_eq!(config.flush_interval, Duration::from_millis(MAX_FLUSH_INTERVAL_MS));
    assert_eq!(config.max_retries, MAX_BULK_RETRIES);
    assert_eq!(config.channel_size, MAX_CHANNEL_SIZE);
}

#[test]
fn test_bulk_config_flush_interval_clamped_to_min() {
    let crd = ElasticsearchClientConfig {
        bulk: Some(EsBulk {
            batch_size: None,
            flush_interval: Some(0),
            max_retries: None,
            backoff_ms: None,
            max_body_bytes: None,
            channel_size: None,
        }),
        ..Default::default()
    };
    let config = EsBulkConfig::from_crd(&crd);
    assert_eq!(config.flush_interval, Duration::from_millis(MIN_FLUSH_INTERVAL_MS));
}

/// Operator-supplied 0 must clamp up to `MIN_CHANNEL_SIZE`; tokio's
/// `mpsc::channel(0)` would otherwise create a rendezvous channel that
/// requires a paired receive call and stalls every producer.
#[test]
fn test_bulk_config_channel_size_clamped_to_min() {
    let crd = ElasticsearchClientConfig {
        bulk: Some(EsBulk {
            batch_size: None,
            flush_interval: None,
            max_retries: None,
            backoff_ms: None,
            max_body_bytes: None,
            channel_size: Some(0),
        }),
        ..Default::default()
    };
    let config = EsBulkConfig::from_crd(&crd);
    assert_eq!(config.channel_size, MIN_CHANNEL_SIZE);
}

#[test]
fn test_bulk_config_channel_size_default() {
    let crd = ElasticsearchClientConfig::default();
    let config = EsBulkConfig::from_crd(&crd);
    assert_eq!(config.channel_size, DEFAULT_CHANNEL_SIZE);
}

#[test]
fn test_current_index_name_format() {
    let config = EsBulkConfig {
        batch_size: 500,
        flush_interval: Duration::from_secs(5),
        max_retries: 3,
        backoff: Duration::from_secs(1),
        max_body_bytes: 10 * 1024 * 1024,
        channel_size: DEFAULT_CHANNEL_SIZE,
        index_prefix: "test-logs".to_string(),
        date_pattern: "%Y.%m.%d".to_string(),
    };
    let name = config.current_index_name();
    assert!(name.starts_with("test-logs-"));
    // Format: test-logs-YYYY.MM.DD (20 chars)
    assert_eq!(name.len(), "test-logs-2026.02.11".len());
}

/// `tls.enabled = false` must NOT inject TLS material even when `verify=false`
/// is set. The `enabled` gate prevents the outbound resolver from being called
/// entirely, so TLS verification is irrelevant.
#[test]
fn test_tls_disabled_ignores_verify_flag() {
    use edgion_resources::resources::common::TlsConfig;

    let crd = ElasticsearchClientConfig {
        tls: Some(TlsConfig {
            enabled: false,
            outbound: edgion_resources::resources::common::OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }),
        ..Default::default()
    };
    let result = build_es_client(&crd, &no_auth());
    assert!(
        result.is_ok(),
        "tls.enabled=false must skip TLS handling entirely, got: {:?}",
        result.err()
    );
}

#[test]
fn test_pool_config_clamped() {
    let crd = ElasticsearchClientConfig {
        pool: Some(EsPool {
            max_idle_per_host: Some(100), // Over max
            idle_timeout: Some(300_000),
        }),
        ..Default::default()
    };
    let (_, _) = build_es_client(&crd, &no_auth()).unwrap();
}

//! Configuration mapping: CRD `ElasticsearchClientConfig` → reqwest `Client` + default headers.
//!
//! Safety ceilings are enforced on all timeout and pool values to prevent misconfiguration.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, ClientBuilder};
use std::time::Duration;

use crate::link_sys::credential::{CredentialState, ResolvedCredential};
use crate::link_sys::tls as tls_helper;
use edgion_resources::resources::link_sys::elasticsearch::*;

// ============================================================================
// Safety ceilings
// ============================================================================

const MAX_CONNECT_TIMEOUT_MS: u64 = 30_000; // 30 seconds
const MAX_REQUEST_TIMEOUT_MS: u64 = 120_000; // 2 minutes
const MAX_IDLE_PER_HOST: usize = 32;
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_IDLE_PER_HOST: usize = 10;
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 90_000;

// ============================================================================
// Bulk configuration safety ceilings
// ============================================================================

const MAX_BATCH_SIZE: usize = 5_000;
const MAX_FLUSH_INTERVAL_MS: u64 = 60_000; // 1 minute
const MAX_BULK_BODY_BYTES: usize = 50 * 1024 * 1024; // 50MB
const MAX_BULK_RETRIES: u32 = 10;
const MAX_CHANNEL_SIZE: usize = 100_000;
const MIN_CHANNEL_SIZE: usize = 1;
const DEFAULT_BATCH_SIZE: usize = 500;
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 5_000;
const DEFAULT_BULK_BODY_BYTES: usize = 10 * 1024 * 1024; // 10MB
const DEFAULT_BULK_RETRIES: u32 = 3;
const DEFAULT_BACKOFF_MS: u64 = 1_000;
const DEFAULT_CHANNEL_SIZE: usize = 10_000;
const DEFAULT_INDEX_PREFIX: &str = "edgion-logs";
const DEFAULT_DATE_PATTERN: &str = "%Y.%m.%d";

// ============================================================================
// Build reqwest Client from CRD config
// ============================================================================

/// Build a reqwest Client and default headers from CRD config.
///
/// The returned `HeaderMap` includes Content-Type and any auth headers.
/// TLS and pool settings are applied to the client builder.
///
/// `auth` is the [`CredentialState`] produced by
/// [`credential::resolved_state`](crate::link_sys::credential::resolved_state).
/// Because `resolved_state` drives resolver selection from the CRD auth
/// variant, a `Resolved(_)` value is guaranteed to match the CRD's auth kind
/// — "type mismatch" is therefore impossible and is not checked here.
/// `Pending` / `Invalid` produce a fail-fast `bail!` (see I4 / M4).
pub fn build_es_client(
    crd: &ElasticsearchClientConfig,
    auth: &CredentialState<ResolvedCredential>,
) -> Result<(Client, HeaderMap)> {
    let mut builder = ClientBuilder::new();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    builder = apply_pool(builder, crd);
    builder = apply_timeouts(builder, crd);
    apply_auth(&mut headers, crd, auth)?;
    builder = builder.default_headers(headers.clone());
    builder = apply_tls(builder, crd)?;

    let client = builder.build().context("failed to build reqwest client")?;
    Ok((client, headers))
}

fn apply_pool(builder: ClientBuilder, crd: &ElasticsearchClientConfig) -> ClientBuilder {
    let idle_per_host = crd
        .pool
        .as_ref()
        .and_then(|p| p.max_idle_per_host)
        .map(|n| n.min(MAX_IDLE_PER_HOST))
        .unwrap_or(DEFAULT_IDLE_PER_HOST);
    let idle_timeout = crd
        .pool
        .as_ref()
        .and_then(|p| p.idle_timeout)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);
    builder
        .pool_max_idle_per_host(idle_per_host)
        .pool_idle_timeout(Duration::from_millis(idle_timeout))
}

fn apply_timeouts(builder: ClientBuilder, crd: &ElasticsearchClientConfig) -> ClientBuilder {
    let connect_timeout = crd
        .timeout
        .as_ref()
        .and_then(|t| t.connect)
        .map(|ms| ms.min(MAX_CONNECT_TIMEOUT_MS))
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);
    let request_timeout = crd
        .timeout
        .as_ref()
        .and_then(|t| t.request)
        .map(|ms| ms.min(MAX_REQUEST_TIMEOUT_MS))
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
    builder
        .connect_timeout(Duration::from_millis(connect_timeout))
        .timeout(Duration::from_millis(request_timeout))
}

fn apply_auth(
    headers: &mut HeaderMap,
    crd: &ElasticsearchClientConfig,
    auth: &CredentialState<ResolvedCredential>,
) -> Result<()> {
    if crd.auth.is_none() {
        return Ok(());
    }
    let resolved = crate::link_sys::credential::require_resolved(auth, "Elasticsearch")?;
    let auth_header = match resolved {
        ResolvedCredential::Basic(m) => {
            let u = m.username.as_deref().unwrap_or("");
            format!("Basic {}", BASE64.encode(format!("{}:{}", u, m.password.as_str())))
        }
        ResolvedCredential::EsApiKey(m) => {
            format!(
                "ApiKey {}",
                BASE64.encode(format!("{}:{}", m.api_key_id, m.api_key_secret))
            )
        }
        ResolvedCredential::Bearer(m) => format!("Bearer {}", m.token),
        ResolvedCredential::Hmac(_) => anyhow::bail!(
            "Elasticsearch auth does not support HMAC material (internal bug: resolver selected wrong variant)"
        ),
        ResolvedCredential::WebhookApiKey(_) => anyhow::bail!(
            "Elasticsearch auth does not support WebhookApiKey material (internal bug: resolver selected wrong variant)"
        ),
    };
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&auth_header).context("invalid auth header value")?,
    );
    Ok(())
}

fn apply_tls(builder: ClientBuilder, crd: &ElasticsearchClientConfig) -> Result<ClientBuilder> {
    let mut builder = builder;
    if let Some(tls) = &crd.tls {
        if tls.enabled {
            builder = tls_helper::apply_insecure_skip_verify(builder, tls, "Elasticsearch")?;
            builder = tls_helper::apply_to_reqwest_builder(builder, tls, "Elasticsearch")?;
        }
    }
    Ok(builder)
}

// ============================================================================
// Resolved Bulk Configuration
// ============================================================================

/// Resolved bulk configuration (from CRD with defaults + ceiling enforcement).
#[derive(Clone, Debug)]
pub struct EsBulkConfig {
    pub batch_size: usize,
    pub flush_interval: Duration,
    pub max_retries: u32,
    pub backoff: Duration,
    pub max_body_bytes: usize,
    /// Capacity of the producer→bulk-loop mpsc channel; clamped to
    /// `[MIN_CHANNEL_SIZE, MAX_CHANNEL_SIZE]` so an operator-supplied 0 cannot
    /// produce a rendezvous channel that would deadlock on every send.
    pub channel_size: usize,
    pub index_prefix: String,
    pub date_pattern: String,
}

impl EsBulkConfig {
    /// Build from CRD config, applying defaults and safety ceilings.
    pub fn from_crd(crd: &ElasticsearchClientConfig) -> Self {
        let bulk = crd.bulk.as_ref();
        let index = crd.index.as_ref();

        Self {
            batch_size: bulk
                .and_then(|b| b.batch_size)
                .map(|s| s.min(MAX_BATCH_SIZE))
                .unwrap_or(DEFAULT_BATCH_SIZE),
            flush_interval: Duration::from_millis(
                bulk.and_then(|b| b.flush_interval)
                    .map(|ms| ms.min(MAX_FLUSH_INTERVAL_MS))
                    .unwrap_or(DEFAULT_FLUSH_INTERVAL_MS),
            ),
            max_retries: bulk
                .and_then(|b| b.max_retries)
                .map(|r| r.min(MAX_BULK_RETRIES))
                .unwrap_or(DEFAULT_BULK_RETRIES),
            backoff: Duration::from_millis(bulk.and_then(|b| b.backoff_ms).unwrap_or(DEFAULT_BACKOFF_MS)),
            max_body_bytes: bulk
                .and_then(|b| b.max_body_bytes)
                .map(|s| s.min(MAX_BULK_BODY_BYTES))
                .unwrap_or(DEFAULT_BULK_BODY_BYTES),
            channel_size: bulk
                .and_then(|b| b.channel_size)
                .map(|s| s.clamp(MIN_CHANNEL_SIZE, MAX_CHANNEL_SIZE))
                .unwrap_or(DEFAULT_CHANNEL_SIZE),
            index_prefix: index
                .and_then(|i| i.prefix.clone())
                .unwrap_or_else(|| DEFAULT_INDEX_PREFIX.to_string()),
            date_pattern: index
                .and_then(|i| i.date_pattern.clone())
                .unwrap_or_else(|| DEFAULT_DATE_PATTERN.to_string()),
        }
    }

    /// Generate index name for the current timestamp.
    /// e.g., "edgion-logs-2026.02.11"
    pub fn current_index_name(&self) -> String {
        let now = chrono::Utc::now();
        let date_suffix = now.format(&self.date_pattern).to_string();
        format!("{}-{}", self.index_prefix, date_suffix)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
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
    }

    /// I4 / M4 regression: `Pending` state → fail-fast.
    #[test]
    fn test_build_es_client_auth_pending_fails_fast() {
        let crd = basic_auth_crd();
        let err = build_es_client(&crd, &CredentialState::Pending).expect_err("should refuse pending");
        assert!(
            err.to_string().contains("has not been resolved"),
            "unexpected error message: {err}"
        );
    }

    /// M4 regression: `Invalid` state → fail-fast with reason surfaced.
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
    }

    #[test]
    fn test_bulk_config_defaults() {
        let crd = ElasticsearchClientConfig::default();
        let config = EsBulkConfig::from_crd(&crd);
        assert_eq!(config.batch_size, DEFAULT_BATCH_SIZE);
        assert_eq!(config.max_retries, DEFAULT_BULK_RETRIES);
        assert_eq!(config.index_prefix, DEFAULT_INDEX_PREFIX);
        assert_eq!(config.date_pattern, DEFAULT_DATE_PATTERN);
    }

    #[test]
    fn test_bulk_config_clamped_to_max() {
        let crd = ElasticsearchClientConfig {
            bulk: Some(EsBulk {
                batch_size: Some(100_000), // Over max
                flush_interval: None,
                max_retries: Some(100), // Over max
                backoff_ms: None,
                max_body_bytes: None,
                channel_size: Some(1_000_000), // Over max
            }),
            ..Default::default()
        };
        let config = EsBulkConfig::from_crd(&crd);
        assert_eq!(config.batch_size, MAX_BATCH_SIZE);
        assert_eq!(config.max_retries, MAX_BULK_RETRIES);
        assert_eq!(config.channel_size, MAX_CHANNEL_SIZE);
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

    /// P1-02: `tls.enabled = false` must NOT inject TLS material or honour
    /// `insecure_skip_verify`. Without this gate ES diverges from Redis/Etcd
    /// and would `bail!` on `insecure_skip_verify: true` in production even
    /// though the operator has not opted into TLS.
    ///
    /// Serialised via `link_sys::tls::ENV_TEST_MUTEX` because this test
    /// mutates `EDGION_ENV` (a process-global table) and would race against
    /// the `apply_insecure_skip_verify_prod_bails_nonprod_accepts` test in
    /// the same test binary otherwise.
    #[test]
    fn test_tls_disabled_ignores_insecure_skip_verify_even_in_prod() {
        use edgion_resources::resources::common::TlsConfig;

        let _env_guard = crate::link_sys::tls::ENV_TEST_MUTEX.lock();

        let crd = ElasticsearchClientConfig {
            tls: Some(TlsConfig {
                enabled: false,
                certs: None,
                insecure_skip_verify: Some(true),
            }),
            ..Default::default()
        };
        let prior = std::env::var("EDGION_ENV").ok();
        // Simulate production: would bail if the `enabled` gate were missing.
        std::env::set_var("EDGION_ENV", "production");
        let result = build_es_client(&crd, &no_auth());
        match prior {
            Some(v) => std::env::set_var("EDGION_ENV", v),
            None => std::env::remove_var("EDGION_ENV"),
        }
        assert!(
            result.is_ok(),
            "tls.enabled=false must skip insecure_skip_verify handling, got: {:?}",
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
}

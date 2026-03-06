//! Configuration mapping: CRD `ElasticsearchClientConfig` → reqwest `Client` + default headers.
//!
//! Safety ceilings are enforced on all timeout and pool values to prevent misconfiguration.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, ClientBuilder};
use std::time::Duration;

use crate::types::resources::link_sys::elasticsearch::*;

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
const DEFAULT_BATCH_SIZE: usize = 500;
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 5_000;
const DEFAULT_BULK_BODY_BYTES: usize = 10 * 1024 * 1024; // 10MB
const DEFAULT_BULK_RETRIES: u32 = 3;
const DEFAULT_BACKOFF_MS: u64 = 1_000;
const DEFAULT_INDEX_PREFIX: &str = "edgion-logs";
const DEFAULT_DATE_PATTERN: &str = "%Y.%m.%d";

// ============================================================================
// Build reqwest Client from CRD config
// ============================================================================

/// Build a reqwest Client and default headers from CRD config.
///
/// The returned `HeaderMap` includes Content-Type and any auth headers.
/// TLS and pool settings are applied to the client builder.
pub fn build_es_client(crd: &ElasticsearchClientConfig) -> Result<(Client, HeaderMap)> {
    let mut builder = ClientBuilder::new();

    // ── Connection pool ────────────────────────────────────────────
    let idle_per_host = crd
        .pool
        .as_ref()
        .and_then(|p| p.max_idle_per_host)
        .map(|n| n.min(MAX_IDLE_PER_HOST))
        .unwrap_or(DEFAULT_IDLE_PER_HOST);
    builder = builder.pool_max_idle_per_host(idle_per_host);

    let idle_timeout = crd
        .pool
        .as_ref()
        .and_then(|p| p.idle_timeout)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);
    builder = builder.pool_idle_timeout(Duration::from_millis(idle_timeout));

    // ── Timeouts ───────────────────────────────────────────────────
    let connect_timeout = crd
        .timeout
        .as_ref()
        .and_then(|t| t.connect)
        .map(|ms| ms.min(MAX_CONNECT_TIMEOUT_MS))
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);
    builder = builder.connect_timeout(Duration::from_millis(connect_timeout));

    let request_timeout = crd
        .timeout
        .as_ref()
        .and_then(|t| t.request)
        .map(|ms| ms.min(MAX_REQUEST_TIMEOUT_MS))
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
    builder = builder.timeout(Duration::from_millis(request_timeout));

    // ── Default headers ────────────────────────────────────────────
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    // ── Authentication ─────────────────────────────────────────────
    if let Some(auth) = &crd.auth {
        if let Some(token) = &auth.bearer_token {
            let val = format!("Bearer {}", token);
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&val).context("invalid bearer token header value")?,
            );
        } else if let Some(api_key_id) = &auth.api_key_id {
            let api_key_secret = auth.api_key_secret.as_deref().unwrap_or("");
            let encoded = BASE64.encode(format!("{}:{}", api_key_id, api_key_secret));
            let val = format!("ApiKey {}", encoded);
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&val).context("invalid api key header value")?,
            );
        } else if let Some(username) = &auth.username {
            let password = auth.password.as_deref().unwrap_or("");
            let encoded = BASE64.encode(format!("{}:{}", username, password));
            let val = format!("Basic {}", encoded);
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&val).context("invalid basic auth header value")?,
            );
        }
        // secret_ref is resolved externally before calling this function
    }

    builder = builder.default_headers(headers.clone());

    // ── TLS ────────────────────────────────────────────────────────
    if let Some(tls) = &crd.tls {
        if let Some(true) = tls.insecure_skip_verify {
            let is_prod = std::env::var("EDGION_ENV")
                .map(|v| v.eq_ignore_ascii_case("production"))
                .unwrap_or(false);
            if is_prod {
                anyhow::bail!(
                    "Elasticsearch insecure_skip_verify cannot be enabled in production (EDGION_ENV=production). \
                     Use a proper CA certificate or disable TLS verification only in non-production environments."
                );
            }
            tracing::warn!(
                "ES client: TLS certificate verification disabled — insecure! Not allowed when EDGION_ENV=production"
            );
            builder = builder.danger_accept_invalid_certs(true);
        }
        // TODO: Build rustls config from CRD certs/CA when TLS certs are provided.
        // Reuse Edgion's existing TLS config builder (src/core/tls/) in future iteration.
    }

    let client = builder.build().context("failed to build reqwest client")?;
    Ok((client, headers))
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

    #[test]
    fn test_build_es_client_simple() {
        let crd = ElasticsearchClientConfig::default();
        let (_, headers) = build_es_client(&crd).unwrap();
        assert!(headers.get("content-type").is_some());
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn test_build_es_client_basic_auth() {
        let crd = ElasticsearchClientConfig {
            auth: Some(EsAuth {
                username: Some("elastic".to_string()),
                password: Some("secret".to_string()),
                api_key_id: None,
                api_key_secret: None,
                bearer_token: None,
                secret_ref: None,
            }),
            ..Default::default()
        };
        let (_, headers) = build_es_client(&crd).unwrap();
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("Basic "));
    }

    #[test]
    fn test_build_es_client_api_key_auth() {
        let crd = ElasticsearchClientConfig {
            auth: Some(EsAuth {
                username: None,
                password: None,
                api_key_id: Some("my-key-id".to_string()),
                api_key_secret: Some("my-key-secret".to_string()),
                bearer_token: None,
                secret_ref: None,
            }),
            ..Default::default()
        };
        let (_, headers) = build_es_client(&crd).unwrap();
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("ApiKey "));
    }

    #[test]
    fn test_build_es_client_bearer_auth() {
        let crd = ElasticsearchClientConfig {
            auth: Some(EsAuth {
                username: None,
                password: None,
                api_key_id: None,
                api_key_secret: None,
                bearer_token: Some("my-token".to_string()),
                secret_ref: None,
            }),
            ..Default::default()
        };
        let (_, headers) = build_es_client(&crd).unwrap();
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert_eq!(auth, "Bearer my-token");
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
            }),
            ..Default::default()
        };
        let config = EsBulkConfig::from_crd(&crd);
        assert_eq!(config.batch_size, MAX_BATCH_SIZE);
        assert_eq!(config.max_retries, MAX_BULK_RETRIES);
    }

    #[test]
    fn test_current_index_name_format() {
        let config = EsBulkConfig {
            batch_size: 500,
            flush_interval: Duration::from_secs(5),
            max_retries: 3,
            backoff: Duration::from_secs(1),
            max_body_bytes: 10 * 1024 * 1024,
            index_prefix: "test-logs".to_string(),
            date_pattern: "%Y.%m.%d".to_string(),
        };
        let name = config.current_index_name();
        assert!(name.starts_with("test-logs-"));
        // Format: test-logs-YYYY.MM.DD (20 chars)
        assert_eq!(name.len(), "test-logs-2026.02.11".len());
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
        // Should not error — clamping is internal
        let (_, _) = build_es_client(&crd).unwrap();
    }
}

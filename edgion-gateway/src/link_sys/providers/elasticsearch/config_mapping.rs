//! Configuration mapping: CRD `ElasticsearchClientConfig` → reqwest `Client` + default headers.
//!
//! Safety ceilings are enforced on all timeout and pool values to prevent misconfiguration.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use std::fmt;
use std::time::Duration;

use crate::link_sys::credential::{CredentialState, ResolvedCredential};
use crate::link_sys::http_client::{
    build_outbound_client_with_tls_selection_and_headers, OutboundClientConfig, OutboundRedirect,
};
use edgion_common::link_sys_core::outbound_tls::PreparedLinkSysTls;
use edgion_resources::resources::link_sys::elasticsearch::*;
use edgion_resources::resources::link_sys::SystemConfig;

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
const MIN_FLUSH_INTERVAL_MS: u64 = 1;
const MAX_FLUSH_INTERVAL_MS: u64 = 60_000; // 1 minute
const MAX_BULK_BODY_BYTES: usize = 50 * 1024 * 1024; // 50MB
const MAX_BULK_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_BULK_RESPONSE_BYTES: usize = 1024 * 1024;
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

/// Build from the actual source, selection and captured inherited generation.
pub(super) fn build_es_client(prepared: &PreparedLinkSysTls) -> Result<Client> {
    let SystemConfig::Elasticsearch(config) = &prepared.owner().spec.config else {
        anyhow::bail!("Elasticsearch client requires an actual Elasticsearch owner");
    };
    let auth = crate::link_sys::credential::resolved_state(prepared.owner());
    let headers = es_default_headers(config, &auth)?;
    prepared.with_tls_selection(|proof, defaults| {
        build_outbound_client_with_tls_selection_and_headers(es_client_config(config), proof, defaults, headers)
            .context("failed to build reqwest client")
    })
}

/// Pure default-header construction shared by owning runtime and auth tests.
///
/// `auth` is the [`CredentialState`] produced by
/// [`credential::resolved_state`](crate::link_sys::credential::resolved_state).
/// Because `resolved_state` drives resolver selection from the CRD auth
/// variant, a `Resolved(_)` value is guaranteed to match the CRD's auth kind
/// — "type mismatch" is therefore impossible and is not checked here.
/// `Pending` / `Invalid` produce a fail-fast `bail!`.
fn es_default_headers(
    crd: &ElasticsearchClientConfig,
    auth: &CredentialState<ResolvedCredential>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    apply_auth(&mut headers, crd, auth)?;
    Ok(headers)
}

fn es_client_config(crd: &ElasticsearchClientConfig) -> OutboundClientConfig {
    let (idle_per_host, idle_timeout) = es_pool(crd);
    let (connect_timeout, request_timeout) = es_timeouts(crd);
    OutboundClientConfig {
        pool_max_idle_per_host: Some(idle_per_host),
        pool_idle_timeout: Some(idle_timeout),
        connect_timeout: Some(connect_timeout),
        total_timeout: Some(request_timeout),
        redirect: OutboundRedirect::Limit(10),
        tls_context: Some("Elasticsearch"),
        ..Default::default()
    }
}

/// Compute the (max-idle-per-host, idle-timeout) pool values from the CRD,
/// applying the same clamps/defaults as before.
pub(crate) fn es_pool(crd: &ElasticsearchClientConfig) -> (usize, Duration) {
    let idle_per_host = crd
        .pool
        .as_ref()
        .and_then(|p| p.max_idle_per_host)
        .map(|n| n.min(MAX_IDLE_PER_HOST))
        .unwrap_or(DEFAULT_IDLE_PER_HOST);
    let idle_timeout_ms = crd
        .pool
        .as_ref()
        .and_then(|p| p.idle_timeout)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);
    (idle_per_host, Duration::from_millis(idle_timeout_ms))
}

/// Compute the (connect, request) timeout values from the CRD, applying the same
/// clamps/defaults as before.
pub(crate) fn es_timeouts(crd: &ElasticsearchClientConfig) -> (Duration, Duration) {
    let connect = crd
        .timeout
        .as_ref()
        .and_then(|t| t.connect)
        .map(|ms| ms.min(MAX_CONNECT_TIMEOUT_MS))
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);
    let request = crd
        .timeout
        .as_ref()
        .and_then(|t| t.request)
        .map(|ms| ms.min(MAX_REQUEST_TIMEOUT_MS))
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
    (Duration::from_millis(connect), Duration::from_millis(request))
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
    };
    let mut value = HeaderValue::from_str(&auth_header).context("invalid auth header value")?;
    value.set_sensitive(true);
    headers.insert(AUTHORIZATION, value);
    Ok(())
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
    pub max_response_bytes: usize,
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
                    .map(|ms| ms.clamp(MIN_FLUSH_INTERVAL_MS, MAX_FLUSH_INTERVAL_MS))
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
            max_response_bytes: bulk
                .and_then(|b| b.max_response_bytes)
                .map(|s| s.clamp(1, MAX_BULK_RESPONSE_BYTES))
                .unwrap_or(DEFAULT_BULK_RESPONSE_BYTES),
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
    pub fn current_index_name(&self) -> Result<String, fmt::Error> {
        let mut name = self.index_prefix.clone();
        name.push('-');
        chrono::Utc::now().format(&self.date_pattern).write_to(&mut name)?;
        Ok(name)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/elasticsearch/config_mapping_test.rs"]
mod tests;

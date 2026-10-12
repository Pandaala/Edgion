//! Elasticsearch client configuration types.
//!
//! Defines the CRD structure for Elasticsearch LinkSys connections.
//! Uses reqwest HTTP client internally — no `elasticsearch` crate dependency.
//! Compatible with Elasticsearch 7.x / 8.x and OpenSearch.

use std::fmt;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{SecretSlot, TlsConfig};

/// Elasticsearch client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ElasticsearchClientConfig {
    /// Elasticsearch endpoints (e.g., "https://es-node:9200").
    /// Multiple endpoints for round-robin load balancing.
    pub endpoints: Vec<String>,

    /// Authentication configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ElasticsearchAuth>,

    /// TLS configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    /// Timeout configuration (in milliseconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<EsTimeout>,

    /// Connection pool configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<EsPool>,

    /// Bulk ingest configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bulk: Option<EsBulk>,

    /// Index naming configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<EsIndex>,
}

impl ElasticsearchClientConfig {
    /// Validate that the explicit date pattern can format a UTC timestamp.
    ///
    /// Formatting is required in addition to parsing: Chrono accepts some
    /// parsing-only directives, such as `%#z`, that cannot produce output.
    /// This cold check discards output rather than constructing an index name.
    pub fn validate_date_pattern(&self) -> Result<(), &'static str> {
        let Some(pattern) = self.index.as_ref().and_then(|index| index.date_pattern.as_deref()) else {
            return Ok(());
        };
        let timestamp = DateTime::<Utc>::default();
        timestamp
            .format(pattern)
            .write_to(&mut DiscardFormatOutput)
            .map_err(|_| "Elasticsearch index.datePattern must be a strftime pattern that supports UTC formatting")
    }
}

struct DiscardFormatOutput;

impl fmt::Write for DiscardFormatOutput {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Ok(())
    }
}

/// Elasticsearch authentication configuration.
///
/// Uses `type` field to select auth method. All methods require a Secret reference.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ElasticsearchAuth {
    /// Basic authentication (username + password in Secret)
    Basic {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// API Key authentication (apiKeyId + apiKeySecret in Secret)
    ApiKey {
        #[serde(flatten)]
        slot: SecretSlot,
    },
    /// Bearer token authentication (bearerToken in Secret)
    Bearer {
        #[serde(flatten)]
        slot: SecretSlot,
    },
}

/// Elasticsearch timeout configuration (in milliseconds)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsTimeout {
    /// Connection timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<u64>,

    /// Request timeout in milliseconds (per request)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u64>,
}

/// Elasticsearch connection pool configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsPool {
    /// Maximum idle connections per host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_idle_per_host: Option<usize>,

    /// Idle connection timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<u64>,
}

/// Elasticsearch bulk ingest configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsBulk {
    /// Maximum documents per bulk request (default: 500)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<usize>,

    /// Flush interval in milliseconds, even if batch is not full (default: 5000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_interval: Option<u64>,

    /// Maximum retries for transport failures, HTTP 5xx responses, and
    /// item-level 429 rejections, shared by the whole sub-batch (default: 3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Initial backoff in milliseconds for retry (default: 1000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,

    /// Maximum bulk request body size in bytes (default: 10MB, ceiling: 50MB).
    /// A batch whose NDJSON body would exceed this is split into multiple bulk
    /// requests; a single document that on its own exceeds the limit is dropped
    /// with a warning, since it can never be batched safely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<usize>,

    /// Capacity of the in-memory mpsc channel between log producers and the
    /// bulk ingest task (default: 10000, ceiling: 100000). A full channel drops
    /// non-blocking access-log submissions; awaiting producers such as backfill
    /// apply backpressure instead. Raise this to absorb temporary ES slowdowns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_size: Option<usize>,
}

/// Elasticsearch index naming configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsIndex {
    /// Index name prefix (default: "edgion-logs")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Date pattern for time-based indices (UTC strftime format, default: "%Y.%m.%d").
    /// Must support formatting, not only parsing.
    /// Produces indices like "edgion-logs-2026.02.11"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_pattern: Option<String>,
}

// ============================================================================
// Default helpers (for tests)
// ============================================================================

impl Default for ElasticsearchClientConfig {
    fn default() -> Self {
        Self {
            endpoints: vec!["http://localhost:9200".to_string()],
            auth: None,
            tls: None,
            timeout: None,
            pool: None,
            bulk: None,
            index: None,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../../tests/unit/resources/link_sys/elasticsearch_test.rs"]
mod tests;

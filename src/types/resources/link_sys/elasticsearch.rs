//! Elasticsearch client configuration types.
//!
//! Defines the CRD structure for Elasticsearch LinkSys connections.
//! Uses reqwest HTTP client internally — no `elasticsearch` crate dependency.
//! Compatible with Elasticsearch 7.x / 8.x and OpenSearch.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::SecretReference;

/// Elasticsearch client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ElasticsearchClientConfig {
    /// Elasticsearch endpoints (e.g., "https://es-node:9200").
    /// Multiple endpoints for round-robin load balancing.
    pub endpoints: Vec<String>,

    /// Authentication configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<EsAuth>,

    /// TLS configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<EsTls>,

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

    /// Failed cache configuration (for when ES is unavailable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_cache: Option<EsFailedCache>,

    /// Compatibility settings (ES vs OpenSearch)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<EsCompatibility>,
}

/// Elasticsearch authentication configuration.
///
/// Supports three modes (pick one):
/// 1. Basic Auth: username + password (or secretRef with password key)
/// 2. API Key: api_key_id + api_key_secret (or secretRef)
/// 3. Bearer Token: bearer_token (or secretRef)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsAuth {
    /// Username for basic authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Password for basic authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// API Key ID (for API Key authentication)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<String>,

    /// API Key Secret (for API Key authentication)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_secret: Option<String>,

    /// Bearer token (for token-based authentication)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,

    /// Secret reference for credentials
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretReference>,
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

    /// Maximum retries for failed bulk requests (default: 3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Initial backoff in milliseconds for retry (default: 1000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,

    /// Maximum bulk request body size in bytes (default: 10MB)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<usize>,
}

/// Elasticsearch index naming configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsIndex {
    /// Index name prefix (default: "edgion-logs")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Date pattern for time-based indices (strftime format, default: "%Y.%m.%d").
    /// Produces indices like "edgion-logs-2026.02.11"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_pattern: Option<String>,
}

/// Elasticsearch failed cache configuration.
/// Defines where to buffer logs when ES is unavailable.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsFailedCache {
    /// Cache type: "localFile" or "redis"
    #[serde(rename = "type")]
    pub cache_type: EsFailedCacheType,

    /// Local file cache configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_file: Option<EsFailedCacheLocalFile>,

    /// Redis cache configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redis: Option<EsFailedCacheRedis>,
}

/// Failed cache type
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum EsFailedCacheType {
    LocalFile,
    Redis,
}

/// Local file failed cache configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsFailedCacheLocalFile {
    /// Directory path for failed cache files
    pub path: String,
}

/// Redis-based failed cache configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsFailedCacheRedis {
    /// Reference to a Redis LinkSys ("namespace/name")
    pub link_sys_ref: String,

    /// Redis list key for failed logs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_key: Option<String>,

    /// Maximum entries in Redis list
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<u64>,
}

/// Elasticsearch TLS configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsTls {
    /// Enable TLS
    #[serde(default)]
    pub enabled: bool,

    /// TLS certificates configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certs: Option<EsTlsCerts>,

    /// Skip certificate verification (insecure, for testing only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_skip_verify: Option<bool>,
}

/// Elasticsearch TLS certificates configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsTlsCerts {
    /// CA certificate (PEM format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<String>,

    /// Client certificate (PEM format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,

    /// Client key (PEM format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,

    /// Secret reference for certificates
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretReference>,
}

/// Elasticsearch compatibility settings
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EsCompatibility {
    /// Vendor: "elasticsearch" (default) or "opensearch"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<EsVendor>,
}

/// Elasticsearch vendor type
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EsVendor {
    Elasticsearch,
    #[serde(alias = "opensearch")]
    OpenSearch,
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
            failed_cache: None,
            compatibility: None,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_simple_config() {
        let yaml = r#"
endpoints:
  - "http://127.0.0.1:9200"
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.endpoints.len(), 1);
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_serde_full_config() {
        let yaml = r#"
endpoints:
  - "https://es-node-1:9200"
  - "https://es-node-2:9200"
auth:
  username: "elastic"
  password: "secret"
tls:
  enabled: true
  insecureSkipVerify: false
timeout:
  connect: 5000
  request: 30000
pool:
  maxIdlePerHost: 10
  idleTimeout: 90000
bulk:
  batchSize: 500
  flushInterval: 5000
  maxRetries: 3
  backoffMs: 1000
  maxBodyBytes: 10485760
index:
  prefix: "edgion-logs"
  datePattern: "%Y.%m.%d"
compatibility:
  vendor: elasticsearch
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.endpoints.len(), 2);
        assert_eq!(
            config.auth.as_ref().unwrap().username.as_deref(),
            Some("elastic")
        );
        assert!(config.tls.as_ref().unwrap().enabled);
        assert_eq!(config.timeout.as_ref().unwrap().connect, Some(5000));
        assert_eq!(config.bulk.as_ref().unwrap().batch_size, Some(500));
        assert_eq!(
            config.index.as_ref().unwrap().prefix.as_deref(),
            Some("edgion-logs")
        );
    }

    #[test]
    fn test_serde_api_key_auth() {
        let yaml = r#"
endpoints:
  - "https://es:9200"
auth:
  apiKeyId: "my-key-id"
  apiKeySecret: "my-key-secret"
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        let auth = config.auth.unwrap();
        assert_eq!(auth.api_key_id.as_deref(), Some("my-key-id"));
        assert_eq!(auth.api_key_secret.as_deref(), Some("my-key-secret"));
    }

    #[test]
    fn test_serde_bearer_auth() {
        let yaml = r#"
endpoints:
  - "https://es:9200"
auth:
  bearerToken: "my-token-123"
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        let auth = config.auth.unwrap();
        assert_eq!(auth.bearer_token.as_deref(), Some("my-token-123"));
    }

    #[test]
    fn test_serde_failed_cache_local() {
        let yaml = r#"
endpoints:
  - "http://localhost:9200"
failedCache:
  type: localFile
  localFile:
    path: "/var/log/edgion/es-failed"
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        let fc = config.failed_cache.unwrap();
        assert_eq!(fc.cache_type, EsFailedCacheType::LocalFile);
        assert_eq!(fc.local_file.unwrap().path, "/var/log/edgion/es-failed");
    }

    #[test]
    fn test_serde_opensearch_compat() {
        let yaml = r#"
endpoints:
  - "https://opensearch:9200"
compatibility:
  vendor: opensearch
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        let compat = config.compatibility.unwrap();
        assert_eq!(compat.vendor, Some(EsVendor::OpenSearch));
    }

    #[test]
    fn test_serde_roundtrip_json() {
        let config = ElasticsearchClientConfig {
            endpoints: vec!["http://localhost:9200".to_string()],
            auth: Some(EsAuth {
                username: Some("elastic".to_string()),
                password: Some("pass".to_string()),
                api_key_id: None,
                api_key_secret: None,
                bearer_token: None,
                secret_ref: None,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let _: ElasticsearchClientConfig = serde_json::from_str(&json).unwrap();
    }
}

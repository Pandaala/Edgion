//! Elasticsearch client configuration types.
//!
//! Defines the CRD structure for Elasticsearch LinkSys connections.
//! Uses reqwest HTTP client internally — no `elasticsearch` crate dependency.
//! Compatible with Elasticsearch 7.x / 8.x and OpenSearch.

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

    /// Date pattern for time-based indices (strftime format, default: "%Y.%m.%d").
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
  type: basic
  secretRef:
    name: es-creds
tls:
  enabled: true
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
  channelSize: 20000
index:
  prefix: "edgion-logs"
  datePattern: "%Y.%m.%d"
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.endpoints.len(), 2);
        match config.auth.as_ref().unwrap() {
            ElasticsearchAuth::Basic { slot } => {
                assert_eq!(slot.secret_ref.name, "es-creds");
            }
            _ => panic!("Expected Basic auth"),
        }
        assert!(config.tls.as_ref().unwrap().enabled);
        assert_eq!(config.timeout.as_ref().unwrap().connect, Some(5000));
        assert_eq!(config.bulk.as_ref().unwrap().batch_size, Some(500));
        assert_eq!(config.bulk.as_ref().unwrap().channel_size, Some(20_000));
        assert_eq!(config.index.as_ref().unwrap().prefix.as_deref(), Some("edgion-logs"));
    }

    #[test]
    fn test_serde_api_key_auth() {
        let yaml = r#"
endpoints:
  - "https://es:9200"
auth:
  type: apiKey
  secretRef:
    name: es-api-key
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        match config.auth.unwrap() {
            ElasticsearchAuth::ApiKey { slot } => {
                assert_eq!(slot.secret_ref.name, "es-api-key");
            }
            _ => panic!("Expected ApiKey auth"),
        }
    }

    #[test]
    fn test_serde_bearer_auth() {
        let yaml = r#"
endpoints:
  - "https://es:9200"
auth:
  type: bearer
  secretRef:
    name: es-bearer-token
"#;
        let config: ElasticsearchClientConfig = serde_yaml::from_str(yaml).unwrap();
        match config.auth.unwrap() {
            ElasticsearchAuth::Bearer { slot } => {
                assert_eq!(slot.secret_ref.name, "es-bearer-token");
            }
            _ => panic!("Expected Bearer auth"),
        }
    }

    #[test]
    fn test_serde_roundtrip_json() {
        use crate::resources::common::{SecretObjectReference, SecretSlot};
        let config = ElasticsearchClientConfig {
            endpoints: vec!["http://localhost:9200".to_string()],
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
        };
        let json = serde_json::to_string(&config).unwrap();
        let _: ElasticsearchClientConfig = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn es_auth_rejects_inline_username() {
        let yaml = "type: basic\nsecretRef:\n  name: s\nusername: leaked";
        let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
    }

    #[test]
    fn es_auth_rejects_inline_password() {
        let yaml = "type: basic\nsecretRef:\n  name: s\npassword: leaked";
        let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
    }

    #[test]
    fn es_auth_rejects_inline_api_key() {
        let yaml = "type: apiKey\nsecretRef:\n  name: s\napiKeyId: leaked";
        let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "inline apiKeyId must be rejected by deny_unknown_fields");
    }

    #[test]
    fn es_auth_rejects_inline_bearer_token() {
        let yaml = "type: bearer\nsecretRef:\n  name: s\nbearerToken: leaked";
        let r: Result<ElasticsearchAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "inline bearerToken must be rejected by deny_unknown_fields");
    }

    #[test]
    fn es_auth_basic_yaml_roundtrip() {
        let yaml = "type: basic\nsecretRef:\n  name: es-creds";
        let a: super::ElasticsearchAuth = serde_yaml::from_str(yaml).unwrap();
        match a {
            super::ElasticsearchAuth::Basic { slot } => assert_eq!(slot.secret_ref.name, "es-creds"),
            _ => panic!("expected Basic"),
        }
    }

    #[test]
    fn es_auth_apikey_yaml_roundtrip() {
        let yaml = "type: apiKey\nsecretRef:\n  name: es-creds";
        let a: super::ElasticsearchAuth = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(a, super::ElasticsearchAuth::ApiKey { .. }));
    }

    #[test]
    fn es_auth_rejects_unknown_field() {
        let yaml = "type: basic\nsecretRef:\n  name: x\nbogus: 1";
        assert!(serde_yaml::from_str::<super::ElasticsearchAuth>(yaml).is_err());
    }
}

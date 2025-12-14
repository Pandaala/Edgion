//! LinkSys resource definition
//!
//! LinkSys is used to connect to external systems like Redis, Etcd, Elasticsearch, Kafka, etc.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// API group for LinkSys
pub const LINK_SYS_GROUP: &str = "edgion.io";

/// Kind for LinkSys
pub const LINK_SYS_KIND: &str = "LinkSys";

/// LinkSys defines connections to external systems
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "LinkSys",
    plural = "linksys",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysSpec {
    /// Type of the system to connect to
    #[serde(rename = "type")]
    pub sys_type: SystemType,

    /// Redis client configuration (only present when type is Redis)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redis: Option<RedisClientConfig>,

    // Future: Add other system configs like Etcd, ES, Kafka, etc.
}

/// System type enumeration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SystemType {
    Redis,
    Etcd,
    Elasticsearch,
    Kafka,
}

/// Redis client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisClientConfig {
    /// Redis server endpoints (e.g., "redis://127.0.0.1:6379")
    pub endpoints: Vec<String>,

    /// Authentication configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<RedisAuth>,

    /// Database number (0-15 for standard Redis)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<i32>,

    /// Timeout configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<RedisTimeout>,

    /// Connection pool configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<RedisPool>,

    /// Retry configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RedisRetry>,

    /// Redis topology configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<RedisTopology>,

    /// TLS configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<RedisTls>,

    /// Observability configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<RedisObservability>,
}

/// Redis authentication configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisAuth {
    /// Username (for Redis 6+ ACL)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Password
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Secret reference for credentials
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretReference>,
}

/// Reference to a Kubernetes Secret
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecretReference {
    /// Name of the secret
    pub name: String,

    /// Namespace of the secret (defaults to LinkSys namespace)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Key in the secret for username
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_key: Option<String>,

    /// Key in the secret for password
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_key: Option<String>,
}

/// Redis timeout configuration (in milliseconds)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisTimeout {
    /// Connection timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<u64>,

    /// Read timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<u64>,

    /// Write timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<u64>,
}

/// Redis connection pool configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisPool {
    /// Maximum pool size
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,

    /// Minimum idle connections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_idle: Option<u32>,
}

/// Redis retry configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisRetry {
    /// Maximum number of retries
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Backoff configuration (e.g., "exponential", "linear", "fixed")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<RedisBackoff>,
}

/// Redis backoff strategy
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisBackoff {
    /// Backoff strategy type
    #[serde(rename = "type")]
    pub backoff_type: String,

    /// Initial delay in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delay: Option<u64>,

    /// Maximum delay in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<u64>,

    /// Multiplier for exponential backoff
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<f64>,
}

/// Redis topology configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisTopology {
    /// Topology mode
    pub mode: RedisTopologyMode,

    /// Sentinel configuration (only for sentinel mode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinel: Option<RedisSentinel>,

    /// Cluster configuration (only for cluster mode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<RedisCluster>,
}

/// Redis topology mode
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RedisTopologyMode {
    Standalone,
    Sentinel,
    Cluster,
}

/// Redis Sentinel configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisSentinel {
    /// Master name in Sentinel configuration
    pub master_name: String,

    /// Sentinel endpoints
    pub sentinels: Vec<String>,

    /// Sentinel password
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Redis Cluster configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisCluster {
    /// Read from replicas
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_from_replicas: Option<bool>,

    /// Maximum redirects
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u32>,
}

/// Redis TLS configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisTls {
    /// Enable TLS
    #[serde(default)]
    pub enabled: bool,

    /// TLS certificates configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certs: Option<RedisTlsCerts>,

    /// Skip certificate verification (insecure, for testing only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_skip_verify: Option<bool>,
}

/// Redis TLS certificates configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisTlsCerts {
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

/// Redis observability configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisObservability {
    /// Enable metrics collection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<RedisMetrics>,

    /// Logging configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<RedisLogging>,
}

/// Redis metrics configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisMetrics {
    /// Enable metrics
    #[serde(default)]
    pub enabled: bool,

    /// Metrics labels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

/// Redis logging configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisLogging {
    /// Enable logging
    #[serde(default)]
    pub enabled: bool,

    /// Log level (e.g., "debug", "info", "warn", "error")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,

    /// Log slow operations (threshold in milliseconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_log_threshold: Option<u64>,
}


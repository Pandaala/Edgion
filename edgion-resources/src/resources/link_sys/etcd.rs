//! Etcd client configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::resources::common::{SecretSlot, TlsConfig};

/// Etcd client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdClientConfig {
    /// etcd server endpoints (e.g., "http://127.0.0.1:2379", "https://etcd.example.com:2379")
    pub endpoints: Vec<String>,

    /// Authentication configuration (username/password)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<EtcdAuth>,

    /// TLS/SSL configuration for secure connections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    /// Timeout configuration (dial, request, keep-alive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<EtcdTimeout>,

    /// HTTP/2 keep-alive configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<EtcdKeepAlive>,

    /// Namespace prefix for all keys (etcd v3 feature)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Auto-sync cluster members interval (in seconds, 0 to disable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_sync_interval: Option<u64>,

    /// Maximum message size for gRPC calls (in bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_send_size: Option<usize>,

    /// Maximum message size for gRPC responses (in bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_recv_size: Option<usize>,

    /// User agent string for client identification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,

    /// Reject connections to old cluster versions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_old_cluster: Option<bool>,

    /// Observability configuration (logging, metrics)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<EtcdObservability>,
}

/// Etcd authentication configuration.
///
/// Credentials must be stored in a Kubernetes Secret and referenced via `secretRef`.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EtcdAuth {
    /// Embedded Secret slot (`secretRef` + resolved `secret`).
    /// `#[serde(flatten)]` keeps the YAML form unchanged at the parent level.
    #[serde(flatten)]
    pub slot: SecretSlot,
}

/// Etcd timeout configuration (in milliseconds)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdTimeout {
    /// Dial timeout in milliseconds (connection establishment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial: Option<u64>,

    /// Request timeout in milliseconds (per-request)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u64>,

    /// Keep-alive timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u64>,
}

/// Etcd HTTP/2 keep-alive configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdKeepAlive {
    /// Keep-alive time interval in seconds (time between pings)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,

    /// Keep-alive timeout in seconds (max time waiting for ping ack)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Permit keep-alive pings without active streams
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permit_without_stream: Option<bool>,
}

/// Etcd observability configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdObservability {
    /// Metrics configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<EtcdMetrics>,

    /// Logging configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<EtcdLogging>,
}

/// Etcd metrics configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdMetrics {
    /// Enable metrics collection
    #[serde(default)]
    pub enabled: bool,

    /// Metrics labels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

/// Etcd logging configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EtcdLogging {
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[path = "../../../tests/unit/resources/link_sys/etcd_test.rs"]
mod tests;

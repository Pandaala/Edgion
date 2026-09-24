//! Kafka client configuration types for the LinkSys CRD.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{SecretSlot, TlsConfig};

pub const DEFAULT_KAFKA_CHANNEL_SIZE: usize = 4096;
pub const DEFAULT_KAFKA_MAX_TOPICS: usize = 64;
pub const DEFAULT_KAFKA_MAX_PENDING_RECORDS: usize = 4096;
pub const DEFAULT_KAFKA_MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_KAFKA_LINGER_MS: u64 = 5;

/// Kafka client configuration.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KafkaClientConfig {
    /// Kafka broker addresses, e.g. ["broker1:9092", "broker2:9092"]
    pub brokers: Vec<String>,

    /// SASL/PLAIN authentication (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sasl: Option<KafkaSaslConfig>,

    /// TLS configuration (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    /// Per-topic mpsc channel buffer size (default: 4096)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_size: Option<usize>,

    /// Maximum number of topic producer queues owned by this client (default: 64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_topics: Option<usize>,

    /// Maximum records queued or being produced across all topics (default: 4096).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pending_records: Option<usize>,

    /// Maximum UTF-8 payload bytes queued or being produced across all topics
    /// (default: 16777216 / 16 MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pending_bytes: Option<usize>,

    /// BatchProducer linger time in ms (default: 5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linger_ms: Option<u64>,
}

/// SASL/PLAIN authentication configuration.
///
/// The Controller resolves the K8s Secret and injects the value before
/// conf-syncing to the gateway.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KafkaSaslConfig {
    pub username: String,
    /// K8s Secret containing the SASL/PLAIN password.
    pub password: SecretSlot,
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/link_sys/kafka_test.rs"]
mod tests;

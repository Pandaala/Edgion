//! Kafka client configuration types for the LinkSys CRD.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{SecretSlot, TlsConfig};

pub const DEFAULT_KAFKA_CHANNEL_SIZE: usize = 4096;
pub const DEFAULT_KAFKA_MAX_TOPICS: usize = 64;
pub const DEFAULT_KAFKA_MAX_PENDING_RECORDS: usize = 4096;
pub const DEFAULT_KAFKA_MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_KAFKA_LINGER_MS: u64 = 5;

/// Per-produce deadline: rskafka retry backoff has no deadline of its own.
/// The Gateway exits a wedged producer task so it can be recreated on the
/// current client. Linger must be shorter to leave time for production.
pub const KAFKA_PRODUCE_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// BatchProducer linger time in ms (default: 5); must be below the produce timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linger_ms: Option<u64>,
}

impl KafkaClientConfig {
    /// Pure scalar admission shared by Controller and Gateway construction.
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.brokers.is_empty() {
            errors.push("Kafka configuration has no brokers".to_string());
        }
        if self.channel_size == Some(0) {
            errors.push("Kafka channelSize must be greater than zero".to_string());
        }
        if self.max_topics == Some(0) {
            errors.push("Kafka maxTopics must be greater than zero".to_string());
        }
        if self.max_pending_records == Some(0) {
            errors.push("Kafka maxPendingRecords must be greater than zero".to_string());
        }
        if self.max_pending_bytes == Some(0) {
            errors.push("Kafka maxPendingBytes must be greater than zero".to_string());
        }
        if Duration::from_millis(self.linger_ms.unwrap_or(DEFAULT_KAFKA_LINGER_MS)) >= KAFKA_PRODUCE_TIMEOUT {
            errors.push(format!(
                "Kafka lingerMs must be shorter than the {} second produce timeout",
                KAFKA_PRODUCE_TIMEOUT.as_secs()
            ));
        }
        errors
    }
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

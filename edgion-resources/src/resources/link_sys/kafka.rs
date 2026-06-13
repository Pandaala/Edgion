//! Kafka client configuration types for the LinkSys CRD.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::{SecretSlot, TlsConfig};

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
mod tests {
    use super::*;

    #[test]
    fn test_kafka_client_config_brokers_only_serde() {
        let config = KafkaClientConfig {
            brokers: vec!["broker1:9092".to_string(), "broker2:9092".to_string()],
            sasl: None,
            tls: None,
            channel_size: None,
            linger_ms: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        // Optional fields are skipped
        assert!(json.contains("broker1:9092"));
        assert!(!json.contains("sasl"));
        assert!(!json.contains("tls"));
    }
}

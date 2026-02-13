//! LinkSys resource definition
//!
//! LinkSys is used to connect to external systems like Redis, Etcd, Elasticsearch, Kafka, etc.

use super::common::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Submodules
pub mod common;
pub mod elasticsearch;
pub mod etcd;
pub mod redis;
pub mod webhook;

// Re-export common types for convenient access
pub use common::SecretReference;
pub use elasticsearch::ElasticsearchClientConfig;
pub use etcd::EtcdClientConfig;
pub use redis::RedisClientConfig;
pub use webhook::WebhookServiceConfig;

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
    namespaced,
    status = "LinkSysStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysSpec {
    /// System configuration
    #[serde(flatten)]
    pub config: SystemConfig,
}

/// System configuration enum
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(tag = "type", content = "config", rename_all = "lowercase")]
pub enum SystemConfig {
    /// Redis client configuration
    Redis(RedisClientConfig),
    /// Etcd client configuration
    Etcd(EtcdClientConfig),
    /// Elasticsearch client configuration (future)
    Elasticsearch(ElasticsearchClientConfig),
    /// Kafka client configuration (future)
    Kafka(KafkaClientConfig),
    /// HTTP webhook service configuration
    Webhook(WebhookServiceConfig),
}

// Placeholder types for future implementations

/// Kafka client configuration (placeholder)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct KafkaClientConfig {
    /// Kafka brokers
    pub brokers: Vec<String>,
}

impl SystemConfig {
    /// Get the system type
    pub fn system_type(&self) -> SystemType {
        match self {
            SystemConfig::Redis(_) => SystemType::Redis,
            SystemConfig::Etcd(_) => SystemType::Etcd,
            SystemConfig::Elasticsearch(_) => SystemType::Elasticsearch,
            SystemConfig::Kafka(_) => SystemType::Kafka,
            SystemConfig::Webhook(_) => SystemType::Webhook,
        }
    }

    /// Get Webhook configuration if this is a Webhook system
    pub fn as_webhook(&self) -> Option<&WebhookServiceConfig> {
        match self {
            SystemConfig::Webhook(config) => Some(config),
            _ => None,
        }
    }

    /// Get Redis configuration if this is a Redis system
    pub fn as_redis(&self) -> Option<&RedisClientConfig> {
        match self {
            SystemConfig::Redis(config) => Some(config),
            _ => None,
        }
    }

    /// Get Etcd configuration if this is an Etcd system
    pub fn as_etcd(&self) -> Option<&EtcdClientConfig> {
        match self {
            SystemConfig::Etcd(config) => Some(config),
            _ => None,
        }
    }

    /// Get Elasticsearch configuration if this is an Elasticsearch system
    pub fn as_elasticsearch(&self) -> Option<&ElasticsearchClientConfig> {
        match self {
            SystemConfig::Elasticsearch(config) => Some(config),
            _ => None,
        }
    }
}

/// System type enumeration (for helper methods and logging)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SystemType {
    Redis,
    Etcd,
    Elasticsearch,
    Kafka,
    Webhook,
}

impl LinkSys {
    /// Validate configuration based on system type (called during pre_parse)
    pub fn validate_config(&self) {
        let key_name = self.key_name_str();
        match &self.spec.config {
            SystemConfig::Redis(redis_config) => {
                // Validate endpoints
                if redis_config.endpoints.is_empty() {
                    tracing::warn!("LinkSys {}: Redis configuration has no endpoints", key_name);
                }

                // Validate topology consistency
                if let Some(topology) = &redis_config.topology {
                    match topology.mode {
                        redis::RedisTopologyMode::Sentinel => {
                            if topology.sentinel.is_none() {
                                tracing::warn!(
                                    "LinkSys {}: Sentinel mode specified but sentinel config is missing",
                                    key_name
                                );
                            }
                        }
                        redis::RedisTopologyMode::Cluster => {
                            if topology.cluster.is_none() {
                                tracing::warn!(
                                    "LinkSys {}: Cluster mode specified but cluster config is missing",
                                    key_name
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            SystemConfig::Etcd(etcd_config) => {
                // Validate endpoints
                if etcd_config.endpoints.is_empty() {
                    tracing::warn!("LinkSys {}: Etcd configuration has no endpoints", key_name);
                }
            }
            SystemConfig::Elasticsearch(es_config) => {
                // Validate endpoints
                if es_config.endpoints.is_empty() {
                    tracing::warn!(
                        "LinkSys {}: Elasticsearch configuration has no endpoints",
                        key_name
                    );
                }
            }
            SystemConfig::Webhook(webhook_config) => {
                if let Some(err) = webhook_config.get_validation_error() {
                    tracing::warn!("LinkSys {}: Webhook configuration error: {}", key_name, err);
                }
            }
            _ => {
                tracing::warn!(
                    "LinkSys {}: System type {:?} is not yet fully implemented",
                    key_name,
                    self.spec.config.system_type()
                );
            }
        }
    }

    /// Get key name string (namespace/name format)
    fn key_name_str(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

// ============================================================================
// LinkSys Status
// ============================================================================

/// LinkSysStatus describes the status of the LinkSys resource
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysStatus {
    /// Conditions describe the current conditions of the LinkSys resource.
    /// Standard conditions: Accepted, Ready
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

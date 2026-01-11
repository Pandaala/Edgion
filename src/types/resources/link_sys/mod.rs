//! LinkSys resource definition
//!
//! LinkSys is used to connect to external systems like Redis, Etcd, Elasticsearch, Kafka, etc.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Submodules
pub mod common;
pub mod etcd;
pub mod redis;

// Re-export common types for convenient access
pub use common::SecretReference;
pub use etcd::EtcdClientConfig;
pub use redis::RedisClientConfig;

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
}

// Placeholder types for future implementations
/// Elasticsearch client configuration (placeholder)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct ElasticsearchClientConfig {
    /// Elasticsearch endpoints
    pub endpoints: Vec<String>,
}

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
}

/// System type enumeration (for helper methods and logging)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SystemType {
    Redis,
    Etcd,
    Elasticsearch,
    Kafka,
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

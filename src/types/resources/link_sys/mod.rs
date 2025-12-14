//! LinkSys resource definition
//!
//! LinkSys is used to connect to external systems like Redis, Etcd, Elasticsearch, Kafka, etc.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Submodules
pub mod common;
pub mod redis;
pub mod etcd;

// Re-export common types for convenient access
pub use common::SecretReference;
pub use redis::RedisClientConfig;
pub use etcd::EtcdClientConfig;

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

    /// Etcd client configuration (only present when type is Etcd)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etcd: Option<EtcdClientConfig>,

    // Future: Add other system configs like ES, Kafka, etc.
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


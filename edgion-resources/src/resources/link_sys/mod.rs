//! LinkSys resource definition
//!
//! LinkSys is used to connect to external systems like Redis, Etcd, Elasticsearch, Kafka, etc.

use super::common::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Submodules
pub mod common;
pub mod credential_source;
pub mod elasticsearch;
pub mod etcd;
pub mod httpdns;
pub mod kafka;
pub mod redis;
pub mod webhook;

// Re-export common types for convenient access
pub use common::SecretObjectReference;
pub use credential_source::{CredentialSourceConfig, CredentialSourceProvider};
pub use elasticsearch::ElasticsearchClientConfig;
pub use etcd::EtcdClientConfig;
pub use httpdns::HttpDnsResolverConfig;
pub use kafka::KafkaClientConfig;
pub use redis::RedisClientConfig;
pub use webhook::WebhookServiceConfig;

/// LinkSys defines connections to external systems
#[derive(CustomResource, Serialize, Clone, Debug, JsonSchema)]
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

    /// Unrecognized key paths found in the submitted document. Populated only on
    /// operator-authored ingest paths; never serialized and never exposed in the
    /// CRD schema.
    #[serde(skip)]
    #[schemars(skip)]
    pub unknown_fields: Vec<String>,
}

/// Deserialize-only mirror of [`LinkSysSpec`]. `unknown_fields` is derived
/// from the submitted document and therefore is intentionally absent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkSysSpecShadow {
    #[serde(flatten)]
    config: SystemConfig,
}

impl From<LinkSysSpecShadow> for LinkSysSpec {
    fn from(shadow: LinkSysSpecShadow) -> Self {
        Self {
            config: shadow.config,
            unknown_fields: Vec::new(),
        }
    }
}

/// Cached production schema used by all LinkSys unknown-key checks.
pub fn link_sys_spec_schema() -> &'static schemars::Schema {
    static SCHEMA: std::sync::OnceLock<schemars::Schema> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| schemars::schema_for!(LinkSysSpec))
}

impl<'de> Deserialize<'de> for LinkSysSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if !crate::schema_check::strict_ingest_enabled() {
            return Ok(LinkSysSpecShadow::deserialize(deserializer)?.into());
        }
        let value = serde_json::Value::deserialize(deserializer)?;
        let unknown = crate::schema_check::unknown_key_paths(&value, link_sys_spec_schema());
        let mut spec: Self = LinkSysSpecShadow::deserialize(value)
            .map_err(serde::de::Error::custom)?
            .into();
        spec.unknown_fields = unknown;
        Ok(spec)
    }
}

/// System configuration enum
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(tag = "type", content = "config", rename_all = "lowercase")]
pub enum SystemConfig {
    /// Redis client configuration
    Redis(RedisClientConfig),
    /// Etcd client configuration
    Etcd(EtcdClientConfig),
    /// Elasticsearch client configuration
    Elasticsearch(ElasticsearchClientConfig),
    /// Kafka client configuration
    Kafka(KafkaClientConfig),
    /// HTTP webhook service configuration
    Webhook(WebhookServiceConfig),
    /// HTTP DNS resolver configuration
    HttpDns(HttpDnsResolverConfig),
    /// Controller-managed credential acquisition and rotation.
    #[serde(rename = "credentialSource")]
    CredentialSource(CredentialSourceConfig),
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
            SystemConfig::HttpDns(_) => SystemType::HttpDns,
            SystemConfig::CredentialSource(_) => SystemType::CredentialSource,
        }
    }

    /// Get Etcd configuration if this is an Etcd system
    pub fn as_etcd(&self) -> Option<&EtcdClientConfig> {
        match self {
            SystemConfig::Etcd(config) => Some(config),
            _ => None,
        }
    }

    /// Get HTTP DNS configuration if this is an HttpDns system
    pub fn as_http_dns(&self) -> Option<&HttpDnsResolverConfig> {
        match self {
            SystemConfig::HttpDns(config) => Some(config),
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
    HttpDns,
    #[serde(rename = "credentialSource")]
    CredentialSource,
}

impl LinkSys {
    /// Validate the configuration based on system type, returning a list of error messages.
    ///
    /// An empty result means "no problems found". Errors are suitable for inclusion in an
    /// `Accepted=False/reason=Invalid` status condition by the Controller, and are also
    /// logged as warnings by the data-plane `pre_parse` hook (see `impl_resource_meta!`).
    pub fn validate_config(&self) -> Vec<String> {
        let mut errors = Vec::new();
        match &self.spec.config {
            SystemConfig::Redis(redis_config) => {
                if redis_config.endpoints.is_empty() {
                    errors.push("Redis configuration has no endpoints".to_string());
                }

                // Topology consistency
                if let Some(topology) = &redis_config.topology {
                    match topology.mode {
                        redis::RedisTopologyMode::Sentinel => {
                            if topology.sentinel.is_none() {
                                errors.push("Sentinel mode specified but sentinel config is missing".to_string());
                            }
                        }
                        redis::RedisTopologyMode::Cluster => {
                            if topology.cluster.is_none() {
                                errors.push("Cluster mode specified but cluster config is missing".to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            SystemConfig::Etcd(etcd_config) => {
                if etcd_config.endpoints.is_empty() {
                    errors.push("Etcd configuration has no endpoints".to_string());
                }
            }
            SystemConfig::Elasticsearch(es_config) => {
                if es_config.endpoints.is_empty() {
                    errors.push("Elasticsearch configuration has no endpoints".to_string());
                }
            }
            SystemConfig::Webhook(webhook_config) => {
                if let Some(err) = webhook_config.get_validation_error() {
                    errors.push(format!("Webhook configuration error: {}", err));
                }
            }
            SystemConfig::Kafka(kafka_config) => {
                if kafka_config.brokers.is_empty() {
                    errors.push("Kafka configuration has no brokers".to_string());
                }
            }
            SystemConfig::HttpDns(http_dns) => {
                for err in http_dns.validate() {
                    errors.push(format!("HTTP DNS configuration error: {}", err));
                }
            }
            SystemConfig::CredentialSource(config) => {
                errors.extend(config.validation_errors());
            }
        }
        errors
    }

    /// Return the namespace/name key string for this resource.
    pub fn key_name_str(&self) -> String {
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
    /// Standard conditions: Accepted, ResolvedRefs (Gateway API).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_state: Option<CredentialSourceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful_refresh_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_refresh_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desired_consumer_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_consumer_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicted_consumer_count: Option<u32>,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, PartialEq, Eq)]
pub enum CredentialSourceState {
    Idle,
    Valid,
    Expired,
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_link_sys(name: &str, spec: LinkSysSpec) -> LinkSys {
        LinkSys {
            metadata: kube::api::ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    #[test]
    fn http_dns_variant_roundtrips_and_validates() {
        let yaml = r#"
type: httpdns
config:
  urlTemplate: "https://203.0.113.10/d?host={domain}"
  response:
    kind: json
    ipPath: ips
  fallback:
    type: system
"#;
        let spec: LinkSysSpec = serde_yaml::from_str(yaml).expect("parse httpdns LinkSys");
        assert_eq!(spec.config.system_type(), SystemType::HttpDns);
        assert!(spec.config.as_http_dns().is_some());

        let ls = make_link_sys("h", spec);
        assert!(
            ls.validate_config().is_empty(),
            "valid httpdns must pass validate_config"
        );
    }

    #[test]
    fn http_dns_missing_url_template_is_invalid() {
        let yaml = "type: httpdns\nconfig:\n  fallback:\n    type: none\n";
        let spec: LinkSysSpec = serde_yaml::from_str(yaml).unwrap();
        let ls = make_link_sys("h", spec);
        assert!(!ls.validate_config().is_empty());
    }
}

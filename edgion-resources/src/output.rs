//! Log output destination and rotation configuration types
//!
//! Used by the link_sys runtime (`core::gateway::link_sys`) to write
//! observability logs to local files / external sinks.
//!
//! NOTE: This module is unrelated to the `LinkSys` Kubernetes CRD
//! (`types::resources::link_sys`), which describes connections to external
//! systems like Redis/Etcd. For the CRD, use `crate::{LinkSys, LinkSysSpec}`.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to a LinkSys resource by namespace + name.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysRef {
    pub namespace: String,
    pub name: String,
}

impl LinkSysRef {
    /// Returns the "namespace/name" key used by runtime stores.
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

impl fmt::Display for LinkSysRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.namespace, self.name)
    }
}

/// Log file rotation configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RotationConfig {
    /// Rotation strategy
    #[serde(default)]
    pub strategy: RotationStrategy,

    /// Maximum number of rotated files to keep (0 = unlimited)
    #[serde(default)]
    pub max_files: usize,

    /// Interval in seconds to check for rotation (default: 30)
    #[serde(default = "default_check_interval_secs")]
    pub check_interval_secs: u64,
}

fn default_check_interval_secs() -> u64 {
    30
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            strategy: RotationStrategy::default(), // Size(100MB)
            max_files: 10,
            check_interval_secs: default_check_interval_secs(),
        }
    }
}

/// Log rotation strategy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RotationStrategy {
    /// Rotate when file size exceeds limit (in bytes)
    Size(u64),
    /// Rotate daily (at midnight)
    Daily,
    /// Rotate hourly
    Hourly,
    /// Never rotate
    Never,
}

/// Default: 100MB per file
impl Default for RotationStrategy {
    fn default() -> Self {
        Self::Size(100 * 1024 * 1024)
    }
}

/// Local file writer configuration (for YAML/JSON config)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileWriterCfg {
    /// Relative path for log file (e.g. "logs/edgion_access.log")
    #[serde(default = "default_access_log_path")]
    pub path: String,

    /// Queue size for the write queue (optional)
    /// If not set, defaults to `cpu_cores * 10_000`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_size: Option<usize>,

    /// Rotation configuration (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<RotationConfig>,
}

fn default_access_log_path() -> String {
    "logs/edgion_access.log".to_string()
}

impl Default for LocalFileWriterCfg {
    fn default() -> Self {
        Self {
            path: default_access_log_path(),
            queue_size: None,
            rotation: None,
        }
    }
}

/// String output destination configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StringOutput {
    /// Write to local file
    LocalFile(LocalFileWriterCfg),
    /// Ship to Elasticsearch via a LinkSys resource
    #[serde(rename_all = "camelCase")]
    Es { link_sys_ref: LinkSysRef },
    /// Ship to Kafka via a LinkSys resource
    #[serde(rename_all = "camelCase")]
    Kafka { link_sys_ref: LinkSysRef, topic: String },
}

impl Default for StringOutput {
    fn default() -> Self {
        Self::LocalFile(LocalFileWriterCfg::default())
    }
}

/// Runtime local file writer configuration (internal use)
#[derive(Debug, Clone)]
pub struct LocalFileWriterConfig {
    /// Relative path for log file
    pub path: String,
    /// Queue size for the write queue (optional)
    /// If None, will use cpu_cores * 10_000 at runtime
    pub queue_size: Option<usize>,
    /// Rotation configuration
    pub rotation: RotationConfig,
}

impl LocalFileWriterConfig {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            queue_size: None,
            rotation: RotationConfig::default(),
        }
    }

    pub fn with_queue_size(mut self, size: usize) -> Self {
        self.queue_size = Some(size);
        self
    }

    pub fn with_rotation(mut self, rotation: RotationConfig) -> Self {
        self.rotation = rotation;
        self
    }
}

impl From<LocalFileWriterCfg> for LocalFileWriterConfig {
    fn from(cfg: LocalFileWriterCfg) -> Self {
        Self {
            path: cfg.path,
            queue_size: cfg.queue_size,
            rotation: cfg.rotation.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_sys_ref_key() {
        let r = LinkSysRef {
            namespace: "default".to_string(),
            name: "es-main".to_string(),
        };
        assert_eq!(r.key(), "default/es-main");
        assert_eq!(format!("{}", r), "default/es-main");
    }

    #[test]
    fn test_string_output_es_serde_roundtrip_struct() {
        let original = StringOutput::Es {
            link_sys_ref: LinkSysRef {
                namespace: "default".to_string(),
                name: "es-main".to_string(),
            },
        };
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            json,
            r#"{"es":{"linkSysRef":{"namespace":"default","name":"es-main"}}}"#
        );
        let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
        match roundtrip {
            StringOutput::Es { link_sys_ref } => {
                assert_eq!(link_sys_ref.namespace, "default");
                assert_eq!(link_sys_ref.name, "es-main");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_string_output_kafka_serde_roundtrip() {
        let original = StringOutput::Kafka {
            link_sys_ref: LinkSysRef {
                namespace: "default".to_string(),
                name: "kafka-main".to_string(),
            },
            topic: "edgion-access".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            json,
            r#"{"kafka":{"linkSysRef":{"namespace":"default","name":"kafka-main"},"topic":"edgion-access"}}"#
        );
        let roundtrip: StringOutput = serde_json::from_str(&json).unwrap();
        match roundtrip {
            StringOutput::Kafka { link_sys_ref, topic } => {
                assert_eq!(link_sys_ref.key(), "default/kafka-main");
                assert_eq!(topic, "edgion-access");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_string_output_local_file_default_unchanged() {
        let s = StringOutput::default();
        assert!(matches!(s, StringOutput::LocalFile(_)));
    }
}

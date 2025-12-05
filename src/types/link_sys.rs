//! Link system configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            strategy: RotationStrategy::Daily,
            max_files: 7,
        }
    }
}

/// Log rotation strategy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum RotationStrategy {
    /// Rotate daily (at midnight)
    #[default]
    Daily,
    /// Rotate hourly
    Hourly,
    /// Never rotate
    Never,
}

/// Local file writer configuration (for YAML/JSON config)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileWriterCfg {
    /// Relative path for log file (e.g. "logs/edgion_access.log")
    #[serde(default = "default_access_log_path")]
    pub path: String,
    
    /// Buffer size for the write queue (optional)
    /// If not set, defaults to `cpu_cores * 10_000`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_size: Option<usize>,
}

fn default_access_log_path() -> String {
    "logs/edgion_access.log".to_string()
}

impl Default for LocalFileWriterCfg {
    fn default() -> Self {
        Self {
            path: default_access_log_path(),
            queue_size: None,
        }
    }
}

/// String output destination configuration
/// 
/// Currently supports:
/// - LocalFileWriter: write to local file with rotation
/// 
/// Future support:
/// - Elasticsearch
/// - Kafka
/// - ClickHouse
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StringOutput {
    /// Write to local file
    LocalFile(LocalFileWriterCfg),
    // TODO: Es(EsConfig),
    // TODO: Kafka(KafkaConfig),
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
    /// Buffer size for the write queue (optional)
    /// If None, will use default_queue_size() at runtime
    pub queue_size: Option<usize>,
}

impl LocalFileWriterConfig {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            queue_size: None,
        }
    }
    
    pub fn with_queue_size(mut self, size: usize) -> Self {
        self.queue_size = Some(size);
        self
    }
}

impl From<LocalFileWriterCfg> for LocalFileWriterConfig {
    fn from(cfg: LocalFileWriterCfg) -> Self {
        Self {
            path: cfg.path,
            queue_size: cfg.queue_size,
        }
    }
}

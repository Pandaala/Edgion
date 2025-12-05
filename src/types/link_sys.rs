//! Link system configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Directory path for log files
    pub path: String,
    
    /// File name prefix
    #[serde(default = "default_file_prefix")]
    pub file_prefix: String,
    
    /// Rotation configuration
    #[serde(default)]
    pub rotation: RotationConfig,
}

fn default_file_prefix() -> String {
    "access".to_string()
}

impl Default for LocalFileWriterCfg {
    fn default() -> Self {
        Self {
            path: "logs".to_string(),
            file_prefix: default_file_prefix(),
            rotation: RotationConfig::default(),
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
    /// File path or directory for log files
    pub path: PathBuf,
    
    /// File name prefix (used with rotation)
    pub file_prefix: String,
    
    /// Rotation configuration
    pub rotation: RotationConfig,
}

impl LocalFileWriterConfig {
    pub fn new(path: impl Into<PathBuf>, file_prefix: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            file_prefix: file_prefix.into(),
            rotation: RotationConfig::default(),
        }
    }
    
    pub fn with_rotation(mut self, rotation: RotationConfig) -> Self {
        self.rotation = rotation;
        self
    }
}

impl From<LocalFileWriterCfg> for LocalFileWriterConfig {
    fn from(cfg: LocalFileWriterCfg) -> Self {
        Self {
            path: PathBuf::from(cfg.path),
            file_prefix: cfg.file_prefix,
            rotation: cfg.rotation,
        }
    }
}

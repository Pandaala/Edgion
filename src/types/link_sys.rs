//! Link system configuration types

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Log file rotation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationConfig {
    /// Rotation strategy
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RotationStrategy {
    /// Rotate daily (at midnight)
    Daily,
    /// Rotate hourly
    Hourly,
    /// Rotate by file size (in bytes)
    Size(u64),
    /// Never rotate
    Never,
}

/// Local file writer configuration
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


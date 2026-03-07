//! Local file writer implementation of DataSender

mod data_sender_impl;
mod rotation;

use std::path::PathBuf;
use std::sync::mpsc::SyncSender;

use crate::core::common::utils::available_cpu_cores;
use crate::types::link_sys::{LocalFileWriterConfig, RotationConfig};
use crate::types::work_dir;

/// Log type for metrics differentiation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogType {
    Access,
    Ssl,
    Tcp,
    Tls,
    Udp,
}

/// Local file writer that implements DataSender
///
/// Uses a background thread to write log entries (avoids blocking tokio runtime)
/// Supports daily/hourly rotation with automatic cleanup of old files
pub struct LocalFileWriter {
    /// Relative path (will be resolved against work_dir)
    relative_path: String,
    /// Queue size for the write queue
    queue_size: Option<usize>,
    /// Rotation configuration
    pub(super) rotation: RotationConfig,
    /// Log type for metrics differentiation
    pub(super) log_type: LogType,
    pub(super) sender: Option<SyncSender<String>>,
    pub(super) healthy: bool,
}

impl LocalFileWriter {
    /// Create a new LocalFileWriter with the given configuration
    pub fn new(config: LocalFileWriterConfig) -> Self {
        Self {
            relative_path: config.path,
            queue_size: config.queue_size,
            rotation: config.rotation,
            log_type: LogType::Access, // Default to Access
            sender: None,
            healthy: false,
        }
    }

    /// Create with simple relative path
    pub fn with_path(path: impl Into<String>) -> Self {
        Self {
            relative_path: path.into(),
            queue_size: None,
            rotation: RotationConfig::default(),
            log_type: LogType::Access,
            sender: None,
            healthy: false,
        }
    }

    /// Set the log type for metrics differentiation
    pub fn with_log_type(mut self, log_type: LogType) -> Self {
        self.log_type = log_type;
        self
    }

    /// Get full path by resolving relative path against work_dir
    pub(super) fn full_path(&self) -> PathBuf {
        work_dir().resolve(&self.relative_path)
    }

    /// Get the queue size, using default if not configured
    pub(super) fn get_queue_size(&self) -> usize {
        self.queue_size.unwrap_or_else(|| available_cpu_cores() * 10_000)
    }
}

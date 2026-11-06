//! FileSystem-specific configuration types
//!
//! This module contains configuration types specific to the FileSystem backend:
//! - `FileSystemConfig`: Configuration for file-based configuration center

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// FileSystem configuration center settings
///
/// Used when running in file-based mode where configuration is stored as
/// local YAML files. File watching is always enabled in this mode.
///
/// ## Example (YAML)
///
/// ```yaml
/// type: file_system
/// controller_name: edgion.io/gateway-controller
/// conf_dir: /etc/edgion/conf
/// poll_interval_ms: 500
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSystemConfig {
    /// Explicit Gateway API identity of this deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_name: Option<String>,
    /// Directory containing configuration YAML files
    pub conf_dir: PathBuf,
    /// Polling interval in milliseconds for the file watcher.
    /// When absent the watcher uses its built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
    /// Deadline (seconds) for the initial load of every resource kind to reach
    /// readiness. This is the only startup bound; phase waits have no inner
    /// deadline. Must be > 30.
    #[serde(default = "super::readiness::default_cache_ready_deadline_secs")]
    pub cache_ready_deadline_secs: u64,
}

impl Default for FileSystemConfig {
    fn default() -> Self {
        Self {
            controller_name: None,
            conf_dir: PathBuf::from("conf"),
            poll_interval_ms: None,
            cache_ready_deadline_secs: super::readiness::default_cache_ready_deadline_secs(),
        }
    }
}

impl FileSystemConfig {
    /// Set the explicit Gateway API identity of this deployment.
    pub fn with_controller_name(mut self, controller_name: impl Into<String>) -> Self {
        self.controller_name = Some(controller_name.into());
        self
    }

    /// Create a new FileSystemConfig with the given directory
    pub fn new(conf_dir: PathBuf) -> Self {
        Self {
            controller_name: None,
            conf_dir,
            poll_interval_ms: None,
            cache_ready_deadline_secs: super::readiness::default_cache_ready_deadline_secs(),
        }
    }

    /// Get the configuration directory
    pub fn conf_dir(&self) -> &PathBuf {
        &self.conf_dir
    }

    /// Fail-fast validation of startup timing knobs.
    pub fn validate(&self) -> anyhow::Result<()> {
        super::readiness::validate_cache_ready_deadline_secs("file_system", self.cache_ready_deadline_secs)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/config/conf_center/file_system_test.rs"]
mod tests;

//! FileSystem Status Handler
//!
//! Handles resource status persistence for FileSystem mode.
//! Status is written to `.status` files alongside the configuration files.
//!
//! ## File Format
//!
//! For a config file `HTTPRoute_default_my-route.yaml`, the status file is:
//! `HTTPRoute_default_my-route.yaml.status`
//!
//! ## Status Structure
//!
//! Uses standard K8s Condition format for consistency.

use crate::types::resources::common::Condition;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Status file extension
const STATUS_EXTENSION: &str = ".status";

/// Resource status structure (mirrors K8s status format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceStatus {
    /// Standard K8s conditions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl ResourceStatus {
    /// Create a Ready status
    pub fn ready() -> Self {
        Self {
            conditions: vec![Condition {
                type_: "Ready".to_string(),
                status: "True".to_string(),
                reason: "ProcessingComplete".to_string(),
                message: "Resource processed successfully".to_string(),
                last_transition_time: Utc::now().to_rfc3339(),
                observed_generation: None,
            }],
        }
    }

    /// Create an Error status
    pub fn error(reason: &str, message: &str) -> Self {
        Self {
            conditions: vec![Condition {
                type_: "Ready".to_string(),
                status: "False".to_string(),
                reason: reason.to_string(),
                message: message.to_string(),
                last_transition_time: Utc::now().to_rfc3339(),
                observed_generation: None,
            }],
        }
    }

    /// Create a status for YAML parse error
    pub fn parse_error(error: &str) -> Self {
        Self::error("ParseError", error)
    }
}

/// FileSystem status handler
pub struct FileSystemStatusHandler {
    conf_dir: PathBuf,
}

impl FileSystemStatusHandler {
    /// Create a new FileSystemStatusHandler
    pub fn new(conf_dir: PathBuf) -> Self {
        Self { conf_dir }
    }

    /// Get the configuration directory
    pub fn conf_dir(&self) -> &Path {
        &self.conf_dir
    }

    /// Get status file path for a config file
    ///
    /// `aaa.yaml` -> `aaa.yaml.status`
    pub fn status_path(file_path: &Path) -> PathBuf {
        let file_name = file_path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
        file_path.with_file_name(format!("{}{}", file_name, STATUS_EXTENSION))
    }

    /// Build status file path from kind and key
    pub fn build_status_path(&self, kind: &str, key: &str) -> PathBuf {
        let config_path = super::file_watcher::build_path_from_key(&self.conf_dir, kind, key);
        Self::status_path(&config_path)
    }

    /// Check if a path is a status file
    pub fn is_status_file(path: &Path) -> bool {
        path.file_name()
            .map(|s| s.to_string_lossy().ends_with(STATUS_EXTENSION))
            .unwrap_or(false)
    }

    /// Write status to file
    pub fn write_status(&self, kind: &str, key: &str, status: &ResourceStatus) -> std::io::Result<()> {
        let status_path = self.build_status_path(kind, key);
        let json = serde_json::to_string_pretty(status)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&status_path, json)?;

        tracing::trace!(
            component = "fs_status",
            kind = kind,
            key = key,
            status_file = %status_path.display(),
            "Wrote status file"
        );

        Ok(())
    }

    /// Write ready status
    pub fn write_ready(&self, kind: &str, key: &str) -> std::io::Result<()> {
        self.write_status(kind, key, &ResourceStatus::ready())
    }

    /// Write error status
    pub fn write_error(&self, kind: &str, key: &str, reason: &str, message: &str) -> std::io::Result<()> {
        self.write_status(kind, key, &ResourceStatus::error(reason, message))
    }

    /// Delete status file
    pub fn delete_status(&self, kind: &str, key: &str) -> std::io::Result<()> {
        let status_path = self.build_status_path(kind, key);
        if status_path.exists() {
            std::fs::remove_file(&status_path)?;
            tracing::trace!(
                component = "fs_status",
                kind = kind,
                key = key,
                status_file = %status_path.display(),
                "Deleted status file"
            );
        }
        Ok(())
    }

    /// Cleanup orphan status files (status files without corresponding config files)
    pub fn cleanup_orphans(&self) -> std::io::Result<usize> {
        let mut cleaned = 0;

        let entries = match std::fs::read_dir(&self.conf_dir) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(
                    component = "fs_status",
                    conf_dir = %self.conf_dir.display(),
                    error = %e,
                    "Failed to read directory for orphan cleanup"
                );
                return Err(e);
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Only process .status files
            if !Self::is_status_file(&path) {
                continue;
            }

            // Check if corresponding config file exists
            if let Some(config_path) = Self::config_path_from_status(&path) {
                if !config_path.exists() {
                    // Orphan status file - delete it
                    if let Err(e) = std::fs::remove_file(&path) {
                        tracing::warn!(
                            component = "fs_status",
                            status_file = %path.display(),
                            error = %e,
                            "Failed to delete orphan status file"
                        );
                    } else {
                        cleaned += 1;
                        tracing::debug!(
                            component = "fs_status",
                            status_file = %path.display(),
                            "Deleted orphan status file"
                        );
                    }
                }
            }
        }

        if cleaned > 0 {
            tracing::info!(
                component = "fs_status",
                cleaned = cleaned,
                "Cleaned up orphan status files"
            );
        }

        Ok(cleaned)
    }

    /// Get the config file path from a status file path
    ///
    /// `aaa.yaml.status` -> `aaa.yaml`
    fn config_path_from_status(status_path: &Path) -> Option<PathBuf> {
        let file_name = status_path.file_name()?.to_string_lossy();
        file_name
            .strip_suffix(STATUS_EXTENSION)
            .map(|config_name| status_path.with_file_name(config_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_status_path() {
        let path = Path::new("/conf/HTTPRoute_default_my-route.yaml");
        let status = FileSystemStatusHandler::status_path(path);
        assert_eq!(status, Path::new("/conf/HTTPRoute_default_my-route.yaml.status"));
    }

    #[test]
    fn test_is_status_file() {
        assert!(FileSystemStatusHandler::is_status_file(Path::new("test.yaml.status")));
        assert!(!FileSystemStatusHandler::is_status_file(Path::new("test.yaml")));
        assert!(!FileSystemStatusHandler::is_status_file(Path::new("test.status.yaml")));
    }

    #[test]
    fn test_resource_status_serialization() {
        let status = ResourceStatus::ready();
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"Ready\""));
        assert!(json.contains("\"True\""));

        let status = ResourceStatus::error("TestError", "Test message");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"False\""));
        assert!(json.contains("\"TestError\""));
    }

    #[test]
    fn test_write_and_delete_status() {
        let temp_dir = TempDir::new().unwrap();
        let conf_dir = temp_dir.path().to_path_buf();
        let handler = FileSystemStatusHandler::new(conf_dir.clone());

        // Create a test config file
        let config_path = conf_dir.join("HTTPRoute_default_test.yaml");
        std::fs::write(&config_path, "test: content").unwrap();

        // Write ready status
        handler.write_ready("HTTPRoute", "default/test").unwrap();

        let status_path = handler.build_status_path("HTTPRoute", "default/test");
        assert!(status_path.exists());

        // Delete status
        handler.delete_status("HTTPRoute", "default/test").unwrap();
        assert!(!status_path.exists());
    }

    #[test]
    fn test_cleanup_orphans() {
        let temp_dir = TempDir::new().unwrap();
        let conf_dir = temp_dir.path().to_path_buf();
        let handler = FileSystemStatusHandler::new(conf_dir.clone());

        // Create a config file with status
        let config1 = conf_dir.join("HTTPRoute_default_route1.yaml");
        std::fs::write(&config1, "test: content").unwrap();
        handler.write_ready("HTTPRoute", "default/route1").unwrap();

        // Create an orphan status file (no corresponding config)
        let orphan_status = conf_dir.join("HTTPRoute_default_deleted.yaml.status");
        std::fs::write(&orphan_status, r#"{"conditions":[]}"#).unwrap();

        // Verify both exist
        assert!(handler.build_status_path("HTTPRoute", "default/route1").exists());
        assert!(orphan_status.exists());

        // Cleanup orphans
        let cleaned = handler.cleanup_orphans().unwrap();
        assert_eq!(cleaned, 1);

        // Verify: orphan deleted, valid status remains
        assert!(handler.build_status_path("HTTPRoute", "default/route1").exists());
        assert!(!orphan_status.exists());
    }
}

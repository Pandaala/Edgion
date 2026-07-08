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
//! The status file contains the resource's native status in YAML format.
//! For example, HTTPRoute status:
//! ```yaml
//! parents:
//!   - parentRef:
//!       name: my-gateway
//!     controllerName: edgion.io/gateway-controller
//!     conditions:
//!       - type: Accepted
//!         status: "True"
//!         ...
//! ```
//!
//! ## Error Status
//!
//! For resources that fail to parse, a simplified error status is written:
//! ```yaml
//! conditions:
//!   - type: Ready
//!     status: "False"
//!     reason: ParseError
//!     message: "error details..."
//! ```

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use super::super::common::ErrorStatus;
use super::path_registry::PathRegistry;

/// Status file extension
const STATUS_EXTENSION: &str = ".status";

/// FileSystem status handler
pub struct FileSystemStatusHandler {
    conf_dir: PathBuf,
    registry: Option<Arc<PathRegistry>>,
}

impl FileSystemStatusHandler {
    /// Create a new FileSystemStatusHandler
    pub fn new(conf_dir: PathBuf) -> Self {
        Self {
            conf_dir,
            registry: None,
        }
    }

    /// Attach a shared path registry; used by `build_status_path` to resolve the canonical spec file path.
    pub fn with_registry(mut self, registry: Arc<PathRegistry>) -> Self {
        self.registry = Some(registry);
        self
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
        if let Some(reg) = &self.registry {
            let rkey = super::path_registry::ResourceKey::new(kind.to_string(), key.to_string());
            if let Some(spec_path) = reg.winner_of(&rkey) {
                return Self::status_path(&spec_path);
            }
            tracing::warn!(
                component = "fs_status",
                kind = kind,
                key = key,
                "Status write fell back to default path: registry has no winner (race or pre-init?)"
            );
        }
        // Fallback layout MUST match what `FileSystemStorage` writes for new resources
        // (nested namespace subdir), otherwise a first-status write before the watcher
        // observes the spec lands at the flat root while the spec lives nested,
        // leaving a permanent orphan that only `cleanup_orphans` reaps at next start.
        let config_path = super::storage::build_resource_path_from_key(&self.conf_dir, kind, key);
        Self::status_path(&config_path)
    }

    /// Check if a path is a status file
    pub fn is_status_file(path: &Path) -> bool {
        path.file_name()
            .map(|s| s.to_string_lossy().ends_with(STATUS_EXTENSION))
            .unwrap_or(false)
    }

    /// Write native status to file in YAML format
    ///
    /// This writes the resource's native status (e.g., HTTPRouteStatus, GatewayStatus)
    /// directly to the status file, preserving the exact structure.
    ///
    /// # Arguments
    /// * `kind` - Resource kind (e.g., "HTTPRoute")
    /// * `key` - Resource key (e.g., "default/my-route")
    /// * `status` - The native status object (must implement Serialize)
    pub async fn write_native_status<S: Serialize>(&self, kind: &str, key: &str, status: &S) -> std::io::Result<()> {
        let status_path = self.build_status_path(kind, key);
        let yaml =
            serde_yaml::to_string(status).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = status_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&status_path, yaml).await?;

        tracing::trace!(
            component = "fs_status",
            kind = kind,
            key = key,
            status_file = %status_path.display(),
            "Wrote native status file (YAML)"
        );

        Ok(())
    }

    /// Write native status from serde_json::Value
    ///
    /// This is useful when the status is extracted from a generic object
    /// using `extract_status_value()`.
    pub async fn write_status_value(&self, kind: &str, key: &str, status: &serde_json::Value) -> std::io::Result<()> {
        let status_path = self.build_status_path(kind, key);
        let yaml =
            serde_yaml::to_string(status).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = status_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&status_path, yaml).await?;

        tracing::trace!(
            component = "fs_status",
            kind = kind,
            key = key,
            status_file = %status_path.display(),
            "Wrote status value file (YAML)"
        );

        Ok(())
    }

    /// Write error status (for parse errors, etc.)
    ///
    /// This writes a simplified error status when the resource can't be fully parsed.
    pub async fn write_error_status(&self, kind: &str, key: &str, reason: &str, message: &str) -> std::io::Result<()> {
        let status = ErrorStatus::error(reason, message);
        self.write_native_status(kind, key, &status).await
    }

    /// Read status from file as JSON string (for comparison)
    ///
    /// Returns the status content as a JSON string, or None if:
    /// - File doesn't exist
    /// - File read fails
    /// - Parsing/conversion fails
    pub async fn read_status_json(&self, kind: &str, key: &str) -> Option<String> {
        let status_path = self.build_status_path(kind, key);

        match tokio::fs::read_to_string(&status_path).await {
            Ok(yaml_content) => {
                // Parse YAML and convert to JSON for consistent comparison
                match serde_yaml::from_str::<serde_json::Value>(&yaml_content) {
                    Ok(value) => serde_json::to_string(&value).ok(),
                    Err(e) => {
                        tracing::warn!(
                            component = "fs_status",
                            kind = kind,
                            key = key,
                            error = %e,
                            "Failed to parse status file"
                        );
                        None
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(
                    component = "fs_status",
                    kind = kind,
                    key = key,
                    error = %e,
                    "Failed to read status file"
                );
                None
            }
        }
    }

    /// Delete status file
    pub async fn delete_status(&self, kind: &str, key: &str) -> std::io::Result<()> {
        let status_path = self.build_status_path(kind, key);
        match tokio::fs::remove_file(&status_path).await {
            Ok(()) => {
                tracing::trace!(
                    component = "fs_status",
                    kind = kind,
                    key = key,
                    status_file = %status_path.display(),
                    "Deleted status file"
                );
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Cleanup orphan status files (status files without corresponding config files)
    pub async fn cleanup_orphans(&self) -> std::io::Result<usize> {
        use walkdir::WalkDir;
        let conf_dir = self.conf_dir.clone();
        let to_delete = tokio::task::spawn_blocking(move || -> Vec<PathBuf> {
            let mut out = Vec::new();
            for entry in WalkDir::new(&conf_dir).follow_links(false) {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path().to_path_buf();
                if !Self::is_status_file(&path) {
                    continue;
                }
                if let Some(config_path) = Self::config_path_from_status(&path) {
                    if !config_path.exists() {
                        out.push(path);
                    }
                }
            }
            out
        })
        .await
        .map_err(std::io::Error::other)?;

        let mut count = 0usize;
        for path in to_delete {
            match tokio::fs::remove_file(&path).await {
                Ok(_) => {
                    count += 1;
                    tracing::debug!(
                        component = "fs_status",
                        status_file = %path.display(),
                        "Deleted orphan status file"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        component = "fs_status",
                        status_file = %path.display(),
                        error = %e,
                        "Failed to delete orphan status file"
                    );
                }
            }
        }
        if count > 0 {
            tracing::info!(
                component = "fs_status",
                cleaned = count,
                "Cleaned up orphan status files"
            );
        }
        Ok(count)
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

    #[tokio::test]
    async fn test_cleanup_orphans() {
        let temp_dir = TempDir::new().unwrap();
        let conf_dir = temp_dir.path().to_path_buf();
        let handler = FileSystemStatusHandler::new(conf_dir.clone());

        // Create a config file with status under the canonical nested layout —
        // `write_error_status` → `build_status_path` falls back to nested when
        // there is no registry, so the sibling spec must live in the same dir
        // to avoid being seen as orphaned.
        let ns_dir = conf_dir.join("default");
        std::fs::create_dir_all(&ns_dir).unwrap();
        let config1 = ns_dir.join("HTTPRoute_default_route1.yaml");
        std::fs::write(&config1, "test: content").unwrap();
        handler
            .write_error_status("HTTPRoute", "default/route1", "OK", "Success")
            .await
            .unwrap();

        // Create an orphan status file (no corresponding config) — placed at
        // root to verify cleanup_orphans walks the whole tree, not just nested.
        let orphan_status = conf_dir.join("HTTPRoute_default_deleted.yaml.status");
        std::fs::write(&orphan_status, "conditions: []").unwrap();

        // Verify both exist
        assert!(handler.build_status_path("HTTPRoute", "default/route1").exists());
        assert!(orphan_status.exists());

        // Cleanup orphans
        let cleaned = handler.cleanup_orphans().await.unwrap();
        assert_eq!(cleaned, 1);

        // Verify: orphan deleted, valid status remains
        assert!(handler.build_status_path("HTTPRoute", "default/route1").exists());
        assert!(!orphan_status.exists());
    }

    #[tokio::test]
    async fn build_status_path_uses_registry_winner_when_present() {
        use crate::conf_mgr::conf_center::file_system::path_registry::{PathRegistry, ResourceKey};
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let conf = dir.path().to_path_buf();
        let nested = conf.join("gateways");
        std::fs::create_dir_all(&nested).unwrap();
        let spec = nested.join("HTTPRoute_default_r1.yaml");
        std::fs::write(&spec, "x: 1").unwrap();

        let registry = Arc::new(PathRegistry::new());
        registry.record(
            ResourceKey::new("HTTPRoute".to_string(), "default/r1".to_string()),
            spec.clone(),
        );

        let h = FileSystemStatusHandler::new(conf.clone()).with_registry(registry);
        let status_path = h.build_status_path("HTTPRoute", "default/r1");
        assert_eq!(status_path, FileSystemStatusHandler::status_path(&spec));
    }

    #[tokio::test]
    async fn build_status_path_falls_back_to_nested_layout_when_no_winner() {
        // Fallback must match where storage writes new specs (nested namespace
        // subdir), so a first-status race before the watcher observes the spec
        // doesn't leave a flat-root orphan beside a nested spec.
        let dir = TempDir::new().unwrap();
        let conf = dir.path().to_path_buf();
        let h = FileSystemStatusHandler::new(conf.clone());

        let namespaced = h.build_status_path("HTTPRoute", "default/r1");
        assert_eq!(
            namespaced,
            conf.join("default").join("HTTPRoute_default_r1.yaml.status")
        );

        let cluster_scoped = h.build_status_path("GatewayClass", "edgion");
        assert_eq!(
            cluster_scoped,
            conf.join("cluster").join("GatewayClass__edgion.yaml.status")
        );
    }
}

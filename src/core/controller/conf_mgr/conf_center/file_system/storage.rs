//! FileSystemStorage implementation
//!
//! Implements the CenterApi trait using local file system as the backend.
//! Stores resources as YAML files with naming convention: Kind_namespace_name.yaml

use crate::core::controller::conf_mgr::conf_center::traits::{CenterApi, ConfEntry, ConfWriterError, ListOptions, ListResult};
use crate::core::common::utils::extract_resource_metadata;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Format YAML content to pretty format
///
/// This ensures consistent, readable YAML output by:
/// 1. Parsing the input YAML
/// 2. Re-serializing with proper formatting
///
/// If parsing fails, returns the original content unchanged.
fn format_yaml_pretty(content: &str) -> String {
    match serde_yaml::from_str::<serde_yaml::Value>(content) {
        Ok(value) => serde_yaml::to_string(&value).unwrap_or_else(|_| content.to_string()),
        Err(_) => content.to_string(),
    }
}

/// File system based configuration writer
///
/// Stores resources as YAML files with naming convention: Kind_namespace_name.yaml
/// or Kind__name.yaml for cluster-scoped resources.
pub struct FileSystemStorage {
    root: PathBuf,
}

impl FileSystemStorage {
    /// Create a new file system writer with root directory path
    pub fn new<P: Into<PathBuf>>(root: P) -> Self {
        let root_path = root.into();
        let root_abs = if root_path.is_absolute() {
            root_path
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&root_path))
                .unwrap_or(root_path)
        };

        tracing::info!(
            component = "file_system_writer",
            event = "init",
            root = ?root_abs,
            "Initialized FileSystemStorage"
        );

        Self { root: root_abs }
    }

    /// Get the root directory path
    pub fn root(&self) -> &PathBuf {
        &self.root
    }
}

#[async_trait]
impl CenterApi for FileSystemStorage {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| ConfWriterError::IOError(format!("Failed to create directory: {}", e)))?;
        }

        // Format YAML to pretty format for readability
        let formatted_content = format_yaml_pretty(&content);

        fs::write(&path, formatted_content)
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to write file: {}", e)))?;

        tracing::info!(
            component = "file_system_writer",
            event = "resource_set",
            kind = kind,
            namespace = ?namespace,
            name = name,
            path = ?path,
            "Resource written to file"
        );

        Ok(())
    }

    async fn create_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        // Check if file already exists
        if path.exists() {
            return Err(ConfWriterError::AlreadyExists(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        // Delegate to set_one for actual write
        self.set_one(kind, namespace, name, content).await?;

        tracing::info!(
            component = "file_system_writer",
            event = "resource_created",
            kind = kind,
            namespace = ?namespace,
            name = name,
            path = ?path,
            "Resource created on file system"
        );

        Ok(())
    }

    async fn update_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        // Check if file exists
        if !path.exists() {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        // Delegate to set_one for actual write
        self.set_one(kind, namespace, name, content).await?;

        tracing::info!(
            component = "file_system_writer",
            event = "resource_updated",
            kind = kind,
            namespace = ?namespace,
            name = name,
            path = ?path,
            "Resource updated on file system"
        );

        Ok(())
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        if !path.exists() {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read file: {}", e)))?;

        Ok(content)
    }

    async fn list_all(&self, _opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        // FileSystem mode: ignore pagination options, return all items
        let mut resources = Vec::new();
        let mut stack = vec![self.root.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir)
                .await
                .map_err(|e| ConfWriterError::IOError(format!("Failed to read dir: {}", e)))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| ConfWriterError::IOError(format!("Failed to read entry: {}", e)))?
            {
                let path = entry.path();

                if path.is_dir() {
                    stack.push(path);
                    continue;
                }

                // Only process .yaml/.yml files
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if !matches!(ext, "yaml" | "yml") {
                    continue;
                }

                // Read and parse file
                let content = fs::read_to_string(&path)
                    .await
                    .map_err(|e| ConfWriterError::IOError(format!("Failed to read {:?}: {}", path, e)))?;

                // Extract metadata
                if let Some(metadata) = extract_resource_metadata(&content) {
                    resources.push(ConfEntry {
                        kind: metadata.kind.unwrap_or_default(),
                        namespace: metadata.namespace,
                        name: metadata.name.unwrap_or_default(),
                        content,
                    });
                } else {
                    tracing::warn!(
                        component = "file_system_writer",
                        path = ?path,
                        "Failed to extract metadata from file"
                    );
                }
            }
        }

        tracing::info!(
            component = "file_system_writer",
            count = resources.len(),
            "Loaded all resources from file system"
        );

        Ok(ListResult {
            items: resources,
            continue_token: None, // FileSystem does not support pagination
        })
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        if !path.exists() {
            return Err(ConfWriterError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        fs::remove_file(&path)
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to delete file: {}", e)))?;

        tracing::info!(
            component = "file_system_writer",
            event = "resource_deleted",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource deleted from file system"
        );

        Ok(())
    }

    async fn get_list_by_kind(&self, kind: &str, _opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        // FileSystem mode: ignore pagination options, return all items
        let mut resources = Vec::new();
        let prefix = format!("{}_", kind);

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                continue;
            }

            // Only process .yaml/.yml files with matching kind prefix
            if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                if !filename.starts_with(&prefix) {
                    continue;
                }

                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if !matches!(ext, "yaml" | "yml") {
                    continue;
                }

                // Read and parse file
                let content = fs::read_to_string(&path)
                    .await
                    .map_err(|e| ConfWriterError::IOError(format!("Failed to read {:?}: {}", path, e)))?;

                // Extract metadata to verify
                if let Some(metadata) = extract_resource_metadata(&content) {
                    if metadata.kind.as_deref() == Some(kind) {
                        resources.push(ConfEntry {
                            kind: kind.to_string(),
                            namespace: metadata.namespace,
                            name: metadata.name.unwrap_or_default(),
                            content,
                        });
                    }
                }
            }
        }

        tracing::debug!(
            component = "file_system_writer",
            kind = kind,
            count = resources.len(),
            "Loaded resources by kind"
        );

        Ok(ListResult {
            items: resources,
            continue_token: None, // FileSystem does not support pagination
        })
    }

    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        _opts: Option<ListOptions>,
    ) -> Result<ListResult, ConfWriterError> {
        // FileSystem mode: ignore pagination options, return all items
        let mut resources = Vec::new();
        let prefix = format!("{}_{}_", kind, namespace);

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                continue;
            }

            // Only process .yaml/.yml files with matching kind_ns prefix
            if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                if !filename.starts_with(&prefix) {
                    continue;
                }

                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if !matches!(ext, "yaml" | "yml") {
                    continue;
                }

                // Read and parse file
                let content = fs::read_to_string(&path)
                    .await
                    .map_err(|e| ConfWriterError::IOError(format!("Failed to read {:?}: {}", path, e)))?;

                // Extract metadata to verify
                if let Some(metadata) = extract_resource_metadata(&content) {
                    if metadata.kind.as_deref() == Some(kind) && metadata.namespace.as_deref() == Some(namespace) {
                        resources.push(ConfEntry {
                            kind: kind.to_string(),
                            namespace: Some(namespace.to_string()),
                            name: metadata.name.unwrap_or_default(),
                            content,
                        });
                    }
                }
            }
        }

        tracing::debug!(
            component = "file_system_writer",
            kind = kind,
            namespace = namespace,
            count = resources.len(),
            "Loaded resources by kind and namespace"
        );

        Ok(ListResult {
            items: resources,
            continue_token: None, // FileSystem does not support pagination
        })
    }

    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfWriterError> {
        let prefix = format!("{}_", kind);
        let mut count = 0;

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();

            if path.is_dir() {
                continue;
            }

            if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                if filename.starts_with(&prefix) {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if matches!(ext, "yaml" | "yml") {
                        count += 1;
                    }
                }
            }
        }

        Ok(count)
    }

    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfWriterError> {
        let prefix = format!("{}_{}_", kind, namespace);
        let mut count = 0;

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfWriterError::IOError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();

            if path.is_dir() {
                continue;
            }

            if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                if filename.starts_with(&prefix) {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if matches!(ext, "yaml" | "yml") {
                        count += 1;
                    }
                }
            }
        }

        Ok(count)
    }
}

/// Build resource file path: Kind_namespace_name.yaml or Kind__name.yaml
fn build_resource_path(root: &Path, kind: &str, namespace: Option<&str>, name: &str) -> PathBuf {
    let filename = if let Some(ns) = namespace {
        format!("{}_{}_{}.yaml", kind, ns, name)
    } else {
        format!("{}__{}.yaml", kind, name)
    };
    root.join(filename)
}

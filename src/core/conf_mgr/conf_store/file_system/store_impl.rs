use super::FileSystemStore;
use crate::core::conf_mgr::{ConfEntry, ConfStore, ConfStoreError};
use crate::core::utils::extract_resource_metadata;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs;

#[async_trait]
impl ConfStore for FileSystemStore {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfStoreError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        fs::write(&path, content)
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to write file: {}", e)))?;

        tracing::info!(
            component = "file_system_store",
            event = "resource_set",
            kind = kind,
            namespace = ?namespace,
            name = name,
            path = ?path,
            "Resource written to file"
        );

        Ok(())
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfStoreError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        if !path.exists() {
            return Err(ConfStoreError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read file: {}", e)))?;

        Ok(content)
    }

    async fn list_all(&self) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let mut resources = Vec::new();
        let mut stack = vec![self.root.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = fs::read_dir(&dir)
                .await
                .map_err(|e| ConfStoreError::IOError(format!("Failed to read dir: {}", e)))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| ConfStoreError::IOError(format!("Failed to read entry: {}", e)))?
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
                    .map_err(|e| ConfStoreError::IOError(format!("Failed to read {:?}: {}", path, e)))?;

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
                        component = "file_system_store",
                        path = ?path,
                        "Failed to extract metadata from file"
                    );
                }
            }
        }

        tracing::info!(
            component = "file_system_store",
            count = resources.len(),
            "Loaded all resources from file system"
        );

        Ok(resources)
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfStoreError> {
        let path = build_resource_path(&self.root, kind, namespace, name);

        if !path.exists() {
            return Err(ConfStoreError::NotFound(format!(
                "{}/{}/{}",
                kind,
                namespace.unwrap_or("_"),
                name
            )));
        }

        fs::remove_file(&path)
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to delete file: {}", e)))?;

        tracing::info!(
            component = "file_system_store",
            event = "resource_deleted",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource deleted from file system"
        );

        Ok(())
    }

    async fn get_list_by_kind(&self, kind: &str) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let mut resources = Vec::new();
        let prefix = format!("{}_", kind);

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read entry: {}", e)))?
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
                    .map_err(|e| ConfStoreError::IOError(format!("Failed to read {:?}: {}", path, e)))?;

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
            component = "file_system_store",
            kind = kind,
            count = resources.len(),
            "Loaded resources by kind"
        );

        Ok(resources)
    }

    async fn get_list_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let mut resources = Vec::new();
        let prefix = format!("{}_{}_", kind, namespace);

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read entry: {}", e)))?
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
                    .map_err(|e| ConfStoreError::IOError(format!("Failed to read {:?}: {}", path, e)))?;

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
            component = "file_system_store",
            kind = kind,
            namespace = namespace,
            count = resources.len(),
            "Loaded resources by kind and namespace"
        );

        Ok(resources)
    }

    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfStoreError> {
        let prefix = format!("{}_", kind);
        let mut count = 0;

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read entry: {}", e)))?
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

    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfStoreError> {
        let prefix = format!("{}_{}_", kind, namespace);
        let mut count = 0;

        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ConfStoreError::IOError(format!("Failed to read entry: {}", e)))?
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
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

//! FileSystem StatusStore implementation
//!
//! Persists resource status to local JSON files for non-Kubernetes environments.

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

use super::{StatusStore, StatusStoreError};
use crate::types::resources::gateway::GatewayStatus;
use crate::types::resources::http_route::HTTPRouteStatus;

/// Status file wrapper with metadata
#[derive(Debug, Serialize, Deserialize)]
struct StatusFile<T> {
    kind: String,
    namespace: String,
    name: String,
    status: T,
    #[serde(rename = "lastUpdated")]
    last_updated: String,
}

/// FileSystem-based status store
///
/// Stores status as JSON files in the following structure:
/// ```text
/// {root}/
/// ├── Gateway_default_my-gateway.json
/// ├── HTTPRoute_default_my-route.json
/// └── ...
/// ```
pub struct FileSystemStatusStore {
    root: PathBuf,
}

impl FileSystemStatusStore {
    /// Create a new FileSystem status store
    ///
    /// # Arguments
    /// * `root` - Root directory for status files
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
            component = "fs_status_store",
            event = "init",
            root = ?root_abs,
            "Initialized FileSystemStatusStore"
        );

        Self { root: root_abs }
    }

    /// Build file path for a resource status
    fn build_path(&self, kind: &str, namespace: &str, name: &str) -> PathBuf {
        let filename = format!("{}_{}_{}.json", kind, namespace, name);
        self.root.join(filename)
    }

    /// Ensure the status directory exists
    async fn ensure_dir(&self) -> Result<(), StatusStoreError> {
        if !self.root.exists() {
            fs::create_dir_all(&self.root)
                .await
                .map_err(|e| StatusStoreError::IOError(format!("Failed to create status dir: {}", e)))?;
        }
        Ok(())
    }

    /// Write status to file
    async fn write_status<T: Serialize>(
        &self,
        kind: &str,
        namespace: &str,
        name: &str,
        status: T,
    ) -> Result<(), StatusStoreError> {
        self.ensure_dir().await?;

        let status_file = StatusFile {
            kind: kind.to_string(),
            namespace: namespace.to_string(),
            name: name.to_string(),
            status,
            last_updated: Utc::now().to_rfc3339(),
        };

        let json = serde_json::to_string_pretty(&status_file)
            .map_err(|e| StatusStoreError::SerializationError(e.to_string()))?;

        let path = self.build_path(kind, namespace, name);
        fs::write(&path, json)
            .await
            .map_err(|e| StatusStoreError::IOError(format!("Failed to write status file: {}", e)))?;

        tracing::debug!(
            component = "fs_status_store",
            kind = kind,
            namespace = namespace,
            name = name,
            path = ?path,
            "Status written to file"
        );

        Ok(())
    }

    /// Read status from file
    async fn read_status<T: for<'de> Deserialize<'de>>(
        &self,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<Option<T>, StatusStoreError> {
        let path = self.build_path(kind, namespace, name);

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| StatusStoreError::IOError(format!("Failed to read status file: {}", e)))?;

        let status_file: StatusFile<T> =
            serde_json::from_str(&content).map_err(|e| StatusStoreError::SerializationError(e.to_string()))?;

        Ok(Some(status_file.status))
    }
}

#[async_trait]
impl StatusStore for FileSystemStatusStore {
    async fn update_gateway_status(
        &self,
        namespace: &str,
        name: &str,
        status: GatewayStatus,
    ) -> Result<(), StatusStoreError> {
        self.write_status("Gateway", namespace, name, status).await
    }

    async fn update_http_route_status(
        &self,
        namespace: &str,
        name: &str,
        status: HTTPRouteStatus,
    ) -> Result<(), StatusStoreError> {
        self.write_status("HTTPRoute", namespace, name, status).await
    }

    async fn get_gateway_status(&self, namespace: &str, name: &str) -> Result<Option<GatewayStatus>, StatusStoreError> {
        self.read_status("Gateway", namespace, name).await
    }

    async fn get_http_route_status(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Option<HTTPRouteStatus>, StatusStoreError> {
        self.read_status("HTTPRoute", namespace, name).await
    }
}

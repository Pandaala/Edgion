use crate::core::conf_mgr_new::{ConfCenter, ConfWriter, SchemaValidator};
use crate::core::conf_sync::conf_server_new::ConfigSyncServer;
use crate::types::ResourceKind;
use axum::http::StatusCode;
use serde::Serialize;
use std::sync::Arc;

/// Admin state containing ConfCenter and SchemaValidator
pub struct AdminState {
    pub conf_center: Arc<ConfCenter>,
    pub schema_validator: Arc<SchemaValidator>,
}

impl AdminState {
    /// Get the ConfigSyncServer from ConfCenter (may be None if not ready)
    ///
    /// Returns Ok(Arc<ConfigSyncServer>) if ready, Err(StatusCode) if not ready.
    /// Callers should use this method and handle the error appropriately.
    pub fn config_sync_server(&self) -> Result<Arc<ConfigSyncServer>, StatusCode> {
        self.conf_center
            .config_sync_server()
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)
    }

    /// Get the ConfWriter from ConfCenter
    pub fn writer(&self) -> Arc<dyn ConfWriter> {
        self.conf_center.writer()
    }

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.conf_center.is_k8s_mode()
    }

    /// Check if the system is ready (ConfigSyncServer exists)
    #[allow(dead_code)]
    pub fn is_ready(&self) -> bool {
        self.conf_center.is_ready()
    }

    // ==================== Resource Access Methods ====================

    /// List all resources of a kind (returns JSON values)
    pub fn list_resources(&self, kind: ResourceKind) -> Result<Vec<serde_json::Value>, StatusCode> {
        let server = self.config_sync_server()?;
        let kind_str = kind.as_str();

        let result = server.list(kind_str).map_err(|e| {
            tracing::warn!(
                component = "admin_state",
                kind = kind_str,
                error = %e,
                "Failed to list resources"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // Parse JSON data to Vec<Value>
        let values: Vec<serde_json::Value> = serde_json::from_str(&result.data).map_err(|e| {
            tracing::warn!(
                component = "admin_state",
                kind = kind_str,
                error = %e,
                "Failed to parse list response"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        Ok(values)
    }

    /// List resources of a kind in a specific namespace
    pub fn list_resources_namespaced(
        &self,
        kind: ResourceKind,
        namespace: &str,
    ) -> Result<Vec<serde_json::Value>, StatusCode> {
        let all = self.list_resources(kind)?;

        // Filter by namespace
        let filtered = all
            .into_iter()
            .filter(|v| {
                v.get("metadata")
                    .and_then(|m| m.get("namespace"))
                    .and_then(|n| n.as_str())
                    == Some(namespace)
            })
            .collect();

        Ok(filtered)
    }

    /// Get a specific resource by namespace and name
    pub fn get_resource(
        &self,
        kind: ResourceKind,
        namespace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, StatusCode> {
        let all = self.list_resources(kind)?;

        let found = all.into_iter().find(|v| {
            let meta = v.get("metadata");
            let ns_match = meta
                .and_then(|m| m.get("namespace"))
                .and_then(|n| n.as_str())
                == Some(namespace);
            let name_match = meta.and_then(|m| m.get("name")).and_then(|n| n.as_str()) == Some(name);
            ns_match && name_match
        });

        Ok(found)
    }

    /// Get a cluster-scoped resource by name
    pub fn get_cluster_resource(
        &self,
        kind: ResourceKind,
        name: &str,
    ) -> Result<Option<serde_json::Value>, StatusCode> {
        let all = self.list_resources(kind)?;

        let found = all.into_iter().find(|v| {
            v.get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|n| n.as_str())
                == Some(name)
        });

        Ok(found)
    }
}

/// Standard API response format
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    #[allow(dead_code)]
    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

/// List response format
#[derive(Serialize)]
pub struct ListResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<T>>,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ListResponse<T> {
    pub fn success(data: Vec<T>) -> Self {
        let count = data.len();
        Self {
            success: true,
            data: Some(data),
            count,
            error: None,
        }
    }

    #[allow(dead_code)]
    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            count: 0,
            error: Some(message),
        }
    }
}

use crate::core::conf_mgr_new::{CenterApi, ConfCenter, ConfWriterError, SchemaValidator};
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

    /// Get the CenterApi from ConfCenter
    pub fn center_api(&self) -> Arc<dyn CenterApi> {
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

    // ==================== CenterApi Methods (Storage Layer) ====================
    // Used by /api/v1/... endpoints - reads directly from storage

    /// List all resources of a kind from storage (via CenterApi)
    pub async fn center_list_resources(&self, kind: ResourceKind) -> Result<Vec<serde_json::Value>, StatusCode> {
        let api = self.center_api();
        let kind_str = kind.as_str();

        let result = api.get_list_by_kind(kind_str, None).await.map_err(|e| {
            tracing::warn!(component = "admin_state", kind = kind_str, error = %e, "Failed to list from storage");
            map_center_api_error(e)
        })?;

        let values: Vec<serde_json::Value> = result
            .items
            .into_iter()
            .filter_map(|entry| serde_yaml::from_str(&entry.content).or_else(|_| serde_json::from_str(&entry.content)).ok())
            .collect();

        Ok(values)
    }

    /// List resources of a kind in a specific namespace from storage (via CenterApi)
    pub async fn center_list_resources_namespaced(&self, kind: ResourceKind, namespace: &str) -> Result<Vec<serde_json::Value>, StatusCode> {
        let api = self.center_api();
        let kind_str = kind.as_str();

        let result = api.get_list_by_kind_ns(kind_str, namespace, None).await.map_err(|e| {
            tracing::warn!(component = "admin_state", kind = kind_str, namespace = namespace, error = %e, "Failed to list namespaced from storage");
            map_center_api_error(e)
        })?;

        let values: Vec<serde_json::Value> = result
            .items
            .into_iter()
            .filter_map(|entry| serde_yaml::from_str(&entry.content).or_else(|_| serde_json::from_str(&entry.content)).ok())
            .collect();

        Ok(values)
    }

    /// Get a specific resource by namespace and name from storage (via CenterApi)
    pub async fn center_get_resource(&self, kind: ResourceKind, namespace: &str, name: &str) -> Result<Option<serde_json::Value>, StatusCode> {
        let api = self.center_api();
        let kind_str = kind.as_str();

        match api.get_one(kind_str, Some(namespace), name).await {
            Ok(content) => {
                let value: serde_json::Value = serde_yaml::from_str(&content)
                    .or_else(|_| serde_json::from_str(&content))
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                Ok(Some(value))
            }
            Err(ConfWriterError::NotFound(_)) => Ok(None),
            Err(e) => Err(map_center_api_error(e)),
        }
    }

    /// Get a cluster-scoped resource by name from storage (via CenterApi)
    pub async fn center_get_cluster_resource(&self, kind: ResourceKind, name: &str) -> Result<Option<serde_json::Value>, StatusCode> {
        let api = self.center_api();
        let kind_str = kind.as_str();

        match api.get_one(kind_str, None, name).await {
            Ok(content) => {
                let value: serde_json::Value = serde_yaml::from_str(&content)
                    .or_else(|_| serde_json::from_str(&content))
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                Ok(Some(value))
            }
            Err(ConfWriterError::NotFound(_)) => Ok(None),
            Err(e) => Err(map_center_api_error(e)),
        }
    }

    // ==================== Cache Methods (ConfigSyncServer) ====================
    // Used by /configserver/... endpoints - reads from ServerCache

    /// List all resources of a kind from cache
    pub fn cache_list_resources(&self, kind: ResourceKind) -> Result<Vec<serde_json::Value>, StatusCode> {
        let server = self.config_sync_server()?;
        let kind_str = kind.as_str();

        let result = server.list(kind_str).map_err(|e| {
            tracing::warn!(component = "admin_state", kind = kind_str, error = %e, "Failed to list from cache");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        let values: Vec<serde_json::Value> = serde_json::from_str(&result.data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(values)
    }

    /// Get a specific resource by namespace and name from cache
    pub fn cache_get_resource(&self, kind: ResourceKind, namespace: &str, name: &str) -> Result<Option<serde_json::Value>, StatusCode> {
        let all = self.cache_list_resources(kind)?;
        let found = all.into_iter().find(|v| {
            let meta = v.get("metadata");
            let ns_match = meta.and_then(|m| m.get("namespace")).and_then(|n| n.as_str()) == Some(namespace);
            let name_match = meta.and_then(|m| m.get("name")).and_then(|n| n.as_str()) == Some(name);
            ns_match && name_match
        });
        Ok(found)
    }

    /// Get a cluster-scoped resource by name from cache
    pub fn cache_get_cluster_resource(&self, kind: ResourceKind, name: &str) -> Result<Option<serde_json::Value>, StatusCode> {
        let all = self.cache_list_resources(kind)?;
        let found = all.into_iter().find(|v| {
            v.get("metadata").and_then(|m| m.get("name")).and_then(|n| n.as_str()) == Some(name)
        });
        Ok(found)
    }
}

/// Map CenterApi errors to HTTP status codes
fn map_center_api_error(e: ConfWriterError) -> StatusCode {
    match e {
        ConfWriterError::NotFound(_) => StatusCode::NOT_FOUND,
        ConfWriterError::AlreadyExists(_) => StatusCode::CONFLICT,
        ConfWriterError::ValidationError(_) | ConfWriterError::ParseError(_) => StatusCode::BAD_REQUEST,
        ConfWriterError::PermissionDenied(_) => StatusCode::FORBIDDEN,
        ConfWriterError::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
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

use crate::core::conf_mgr::{ConfCenter, ConfWriter, SchemaValidator};
use crate::core::conf_sync::ConfigServer;
use axum::http::StatusCode;
use serde::Serialize;
use std::sync::Arc;

/// Admin state containing ConfCenter and SchemaValidator
pub struct AdminState {
    pub conf_center: Arc<ConfCenter>,
    pub schema_validator: Arc<SchemaValidator>,
}

impl AdminState {
    /// Get the ConfigServer from ConfCenter (may be None if not ready)
    ///
    /// Returns Ok(Arc<ConfigServer>) if ready, Err(StatusCode) if not ready.
    /// Callers should use this method and handle the error appropriately.
    pub fn config_server(&self) -> Result<Arc<ConfigServer>, StatusCode> {
        self.conf_center.config_server().ok_or(StatusCode::SERVICE_UNAVAILABLE)
    }

    /// Get the ConfWriter from ConfCenter
    pub fn writer(&self) -> Arc<dyn ConfWriter> {
        self.conf_center.writer()
    }

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.conf_center.is_k8s_mode()
    }

    /// Check if the system is ready (ConfigServer exists)
    #[allow(dead_code)]
    pub fn is_ready(&self) -> bool {
        self.conf_center.is_ready()
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

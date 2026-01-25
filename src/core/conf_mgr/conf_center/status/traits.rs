//! StatusStore trait definition
//!
//! Provides an abstraction for status storage backends, supporting both
//! Kubernetes (via K8s API) and FileSystem (local files) implementations.

use async_trait::async_trait;
use thiserror::Error;

use crate::types::resources::gateway::GatewayStatus;
use crate::types::resources::http_route::HTTPRouteStatus;

/// Error types for status store operations
#[derive(Debug, Error)]
pub enum StatusStoreError {
    #[error("Resource not found: {kind}/{namespace}/{name}")]
    NotFound {
        kind: String,
        namespace: String,
        name: String,
    },

    #[error("Failed to update status: {0}")]
    UpdateFailed(String),

    #[error("Failed to read status: {0}")]
    ReadFailed(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    IOError(String),

    #[error("Kubernetes API error: {0}")]
    KubeError(String),
}

/// StatusStore trait for persistent status storage
///
/// This trait abstracts the status update mechanism, allowing the same
/// controller logic to work with both Kubernetes and file-based backends.
#[async_trait]
pub trait StatusStore: Send + Sync {
    /// Update Gateway status
    ///
    /// # Arguments
    /// * `namespace` - The namespace of the Gateway
    /// * `name` - The name of the Gateway
    /// * `status` - The new status to set
    async fn update_gateway_status(
        &self,
        namespace: &str,
        name: &str,
        status: GatewayStatus,
    ) -> Result<(), StatusStoreError>;

    /// Update HTTPRoute status
    ///
    /// # Arguments
    /// * `namespace` - The namespace of the HTTPRoute
    /// * `name` - The name of the HTTPRoute
    /// * `status` - The new status to set
    async fn update_http_route_status(
        &self,
        namespace: &str,
        name: &str,
        status: HTTPRouteStatus,
    ) -> Result<(), StatusStoreError>;

    /// Get Gateway status
    ///
    /// Returns None if the status doesn't exist yet.
    async fn get_gateway_status(&self, namespace: &str, name: &str) -> Result<Option<GatewayStatus>, StatusStoreError>;

    /// Get HTTPRoute status
    ///
    /// Returns None if the status doesn't exist yet.
    async fn get_http_route_status(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Option<HTTPRouteStatus>, StatusStoreError>;
}

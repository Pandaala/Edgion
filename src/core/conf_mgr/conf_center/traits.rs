//! ConfWriter trait definition
//!
//! Provides a unified interface for configuration write operations.
//! Implementations: FileSystemWriter (local files), KubernetesWriter (K8s API)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Configuration entry with metadata and content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfEntry {
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub content: String, // Raw YAML content
}

/// Error types for configuration operations
#[derive(Debug, Error)]
pub enum ConfWriterError {
    #[error("Configuration not found: {0}")]
    NotFound(String),

    #[error("Configuration already exists: {0}")]
    AlreadyExists(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("IO error: {0}")]
    IOError(String),

    #[error("Kubernetes API error: {0}")]
    KubeError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Configuration writer trait for persistent configuration storage
///
/// This trait provides a unified interface for both FileSystem and Kubernetes backends.
/// - FileSystem: reads/writes local YAML files
/// - Kubernetes: calls K8s API (similar to client-go)
#[async_trait]
pub trait ConfWriter: Send + Sync {
    /// Set a single configuration (create or update)
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError>;

    /// Get a single configuration YAML content
    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError>;

    /// List all configurations by kind
    async fn get_list_by_kind(&self, kind: &str) -> Result<Vec<ConfEntry>, ConfWriterError>;

    /// List configurations by kind and namespace
    async fn get_list_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<Vec<ConfEntry>, ConfWriterError>;

    /// Count configurations by kind
    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfWriterError>;

    /// Count configurations by kind and namespace
    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfWriterError>;

    /// Delete a single configuration
    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError>;

    /// List all configurations (for initialization)
    async fn list_all(&self) -> Result<Vec<ConfEntry>, ConfWriterError>;
}

// Note: For backward compatibility with old code, use crate::core::conf_mgr::ConfStore
// New code should use ConfWriter directly

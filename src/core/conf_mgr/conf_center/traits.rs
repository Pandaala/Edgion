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

/// List options for pagination (K8s style)
///
/// When `None` is passed to list methods, all items are returned.
/// When `Some(ListOptions)` is passed, pagination is applied.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// Maximum number of items to return (0 = no limit)
    pub limit: u32,
    /// Continue token for pagination (K8s style)
    pub continue_token: Option<String>,
}

/// Result of a list operation with pagination support
#[derive(Debug, Clone)]
pub struct ListResult {
    /// List of configuration entries
    pub items: Vec<ConfEntry>,
    /// Token for fetching next page (None if this is the last page)
    pub continue_token: Option<String>,
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
    ///
    /// # Arguments
    /// * `kind` - Resource kind (e.g., "HTTPRoute", "Gateway")
    /// * `opts` - Pagination options. `None` returns all items, `Some(opts)` applies pagination.
    async fn get_list_by_kind(&self, kind: &str, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError>;

    /// List configurations by kind and namespace
    ///
    /// # Arguments
    /// * `kind` - Resource kind
    /// * `namespace` - Kubernetes namespace
    /// * `opts` - Pagination options. `None` returns all items, `Some(opts)` applies pagination.
    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult, ConfWriterError>;

    /// Count configurations by kind
    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfWriterError>;

    /// Count configurations by kind and namespace
    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfWriterError>;

    /// Delete a single configuration
    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError>;

    /// List all configurations (for initialization)
    ///
    /// # Arguments
    /// * `opts` - Pagination options. `None` returns all items, `Some(opts)` applies pagination.
    async fn list_all(&self, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError>;
}

// Note: For backward compatibility with old code, use crate::core::conf_mgr::ConfStore
// New code should use ConfWriter directly

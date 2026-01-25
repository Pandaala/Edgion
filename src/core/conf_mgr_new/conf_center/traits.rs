//! Configuration Center traits
//!
//! Provides a unified interface for configuration center operations.
//!
//! ## Trait Hierarchy
//!
//! ```text
//! ConfCenter (super trait)
//! ├── CenterApi (CRUD operations)
//! │   └── FileSystemWriter, KubernetesWriter
//! └── CenterLifeCycle (lifecycle management)
//!     └── FileSystemCenter, KubernetesCenter
//! ```
//!
//! ## Usage
//!
//! Implementations should implement both `CenterApi` and `CenterLifeCycle`,
//! then they automatically implement `ConfCenter` via blanket impl.

use crate::core::conf_mgr_new::sync_runtime::ShutdownHandle;
use crate::core::conf_sync::conf_server_new::ConfigSyncServer;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("IO error: {0}")]
    IOError(String),

    #[error("Kubernetes API error: {0}")]
    KubeError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

// ============================================================================
// CenterApi - CRUD operations for configuration storage
// ============================================================================

/// Configuration center API trait for persistent configuration storage
///
/// This trait provides a unified interface for both FileSystem and Kubernetes backends.
/// - FileSystem: reads/writes local YAML files
/// - Kubernetes: calls K8s API (similar to client-go)
#[async_trait]
pub trait CenterApi: Send + Sync {
    /// Set a single configuration (create or update, implementation-specific)
    ///
    /// For FileSystem: always overwrites the file
    /// For Kubernetes: uses server-side apply (patch)
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError>;

    /// Create a new resource (fails if already exists)
    ///
    /// For FileSystem: checks if file exists before writing
    /// For Kubernetes: uses Api::create()
    async fn create_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError>;

    /// Update an existing resource (fails if not exists)
    ///
    /// For FileSystem: checks if file exists before writing
    /// For Kubernetes: uses Api::replace()
    async fn update_one(
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

// ============================================================================
// CenterLifeCycle - Lifecycle management for configuration center
// ============================================================================

/// Configuration center lifecycle trait
///
/// This trait manages the lifecycle of configuration center backends:
/// - Start/stop the configuration watcher
/// - Handle graceful shutdown
/// - Provide readiness status
/// - Access to ConfigSyncServer for gRPC services
#[async_trait]
pub trait CenterLifeCycle: Send + Sync {
    /// Start the configuration center with an external shutdown handle
    ///
    /// This method starts the configuration watcher and waits for shutdown signal.
    /// It blocks until shutdown is requested or an error occurs.
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()>;

    /// Reload all resources (FileSystem mode only)
    ///
    /// Performs a complete reset:
    /// 1. Clear all caches in PROCESSOR_REGISTRY
    /// 2. Set all processors to not ready
    /// 3. Re-run init phase to reload all resources
    async fn reload(&self) -> Result<()>;

    /// Check if the system is ready
    ///
    /// Ready means PROCESSOR_REGISTRY is ready and ConfigSyncServer exists.
    fn is_ready(&self) -> bool;

    /// Get the ConfigSyncServer (may be None if not ready)
    ///
    /// gRPC services should call this method to get the ConfigSyncServer.
    /// When None, they should return UNAVAILABLE/NOT_READY errors.
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>>;

    /// Check if running in Kubernetes mode
    fn is_k8s_mode(&self) -> bool;
}

// ============================================================================
// ConfCenter - Super trait combining CenterApi + CenterLifeCycle
// ============================================================================

/// Configuration center super trait
///
/// This trait combines `CenterApi` (CRUD operations) and `CenterLifeCycle` (lifecycle management).
/// Any type implementing both `CenterApi` and `CenterLifeCycle` automatically implements `ConfCenter`
/// via blanket impl.
///
/// ## Usage
///
/// ```ignore
/// // ConfMgr holds Arc<dyn ConfCenter>
/// let conf_center: Arc<dyn ConfCenter> = ...;
///
/// // Can call CenterApi methods
/// let content = conf_center.get_one("HTTPRoute", Some("default"), "my-route").await?;
///
/// // Can call CenterLifeCycle methods
/// conf_center.start(shutdown_handle).await?;
/// ```
pub trait ConfCenter: CenterApi + CenterLifeCycle {}

/// Blanket implementation: Any type implementing CenterApi + CenterLifeCycle automatically implements ConfCenter
impl<T: CenterApi + CenterLifeCycle> ConfCenter for T {}

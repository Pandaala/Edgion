//! Configuration Center traits
//!
//! Provides a unified interface for configuration center operations.
//!
//! ## Trait Hierarchy
//!
//! ```text
//! ConfCenter (super trait)
//! ├── CenterApi (CRUD operations)
//! │   └── FileSystemStorage, KubernetesStorage
//! └── CenterLifeCycle (lifecycle management)
//!     └── FileSystemCenter, KubernetesCenter
//! ```
//!
//! ## Usage
//!
//! Implementations should implement both `CenterApi` and `CenterLifeCycle`,
//! then they automatically implement `ConfCenter` via blanket impl.

use crate::core::controller::conf_mgr::sync_runtime::ShutdownHandle;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::core::controller::conf_sync::conf_server::ConfigSyncServer;
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

/// Metadata-only key for a Kubernetes-shaped resource. Mirrors `PartialObjectMetadata`.
///
/// Returned by [`CenterApi::list_keys_by_kind`] and the namespaced variant.
/// Frontend reads `record.metadata.name` etc., same path as full objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceKey {
    pub kind: String,
    pub api_version: String,
    pub metadata: ResourceKeyMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceKeyMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub name: String,
    #[serde(rename = "creationTimestamp", skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub annotations: std::collections::BTreeMap<String, String>,
    #[serde(rename = "resourceVersion", skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
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

/// Result of a list operation with pagination support.
///
/// Default `T = ConfEntry` keeps existing callers source-compatible.
#[derive(Debug, Clone)]
pub struct ListResult<T = ConfEntry> {
    pub items: Vec<T>,
    /// Token for fetching next page (None if this is the last page)
    pub continue_token: Option<String>,
}

/// Error types for configuration operations
#[derive(Debug, Error)]
pub enum ConfWriterError {
    // ---- domain-semantic variants (String payload is intentional) ----
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

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Not supported: {0}")]
    NotSupported(String),

    #[error("Stale pagination: {0}")]
    StalePagination(String),

    // ---- infrastructure variants: preserve #[source] for error chain ----
    /// IO error. `context` names the operation; `std::io::ErrorKind` is
    /// accessible via `Error::source()` for retry / observability logic.
    #[error("IO error: {context}")]
    IOError {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// Parse / serialization error. `context` must be a `'static` string
    /// literal; the underlying error (serde_yaml, serde_json, …) is boxed.
    #[error("Parse error: {context}")]
    ParseError {
        context: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Kubernetes client error not mappable to a domain variant.
    /// The original error is type-erased and accessible via `Error::source()`.
    #[error("Kubernetes API error")]
    KubeError(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Etcd client error.
    #[error("Etcd error: {0}")]
    EtcdError(String),
}

impl ConfWriterError {
    /// Convenience constructor for `IOError`.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::IOError {
            context: context.into(),
            source,
        }
    }
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

    /// Delete a single configuration
    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError>;

    /// List all configurations (for initialization)
    ///
    /// # Arguments
    /// * `opts` - Pagination options. `None` returns all items, `Some(opts)` applies pagination.
    async fn list_all(&self, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError>;

    /// List keys (metadata-only) by kind, across all namespaces / cluster-scoped.
    ///
    /// Used by `/api/v1/keys/...` admin endpoints. Backends that cannot
    /// project metadata efficiently return [`ConfWriterError::NotSupported`].
    ///
    /// Default implementation returns `NotSupported`. Backends override this
    /// once they provide a real implementation (Tasks 3 and 5).
    async fn list_keys_by_kind(
        &self,
        kind: &str,
        _opts: Option<ListOptions>,
    ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
        Err(ConfWriterError::NotSupported(format!(
            "list_keys_by_kind not implemented for kind={kind}"
        )))
    }

    /// List keys (metadata-only) by kind within a single namespace.
    ///
    /// Default implementation returns `NotSupported`. Backends override this
    /// once they provide a real implementation (Tasks 3 and 5).
    async fn list_keys_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        _opts: Option<ListOptions>,
    ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
        Err(ConfWriterError::NotSupported(format!(
            "list_keys_by_kind_ns not implemented for kind={kind} namespace={namespace}"
        )))
    }
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
/// - Support reload (re-initialize all processors and stores)
#[async_trait]
pub trait CenterLifeCycle: Send + Sync {
    /// Start the configuration center with an external shutdown handle
    ///
    /// This method starts the configuration watcher and waits for shutdown signal.
    /// It blocks until shutdown is requested or an error occurs.
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()>;

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

    /// Request a reload (re-initialize all processors and stores)
    ///
    /// This triggers a full restart of the configuration center:
    /// 1. Stop current controllers
    /// 2. Clear PROCESSOR_REGISTRY
    /// 3. Create new ConfigSyncServer (new server_id)
    /// 4. Restart controllers (full Init -> InitApply -> InitDone flow)
    ///
    /// Returns Ok(()) if the reload request was accepted (reload happens asynchronously).
    /// Returns Err if the center is not started or not ready.
    fn request_reload(&self) -> Result<(), String>;

    /// Get the ControllerRuntime for this center
    fn runtime(&self) -> Arc<ControllerRuntime>;

    /// Return the FileSystem ConfCenter's PathRegistry, if applicable.
    /// Default `None` for backends that don't track on-disk paths.
    fn conf_dir_registry(
        &self,
    ) -> Option<Arc<crate::core::controller::conf_mgr::conf_center::file_system::PathRegistry>> {
        None
    }
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

// ============================================================================
// Delegation macro for CenterApi boilerplate
// ============================================================================

/// Generate a pure-delegation `impl CenterApi` for a wrapper type.
/// All trait methods are forwarded to `self.$field`.
///
/// # Example
/// ```ignore
/// impl_center_api_delegate!(MyCenter, writer);
/// ```
///
/// Requires `CenterApi`, `ConfWriterError`, `ListOptions`, `ListResult` in scope.
macro_rules! impl_center_api_delegate {
    ($type:ty, $field:ident) => {
        #[async_trait::async_trait]
        impl CenterApi for $type {
            async fn set_one(
                &self,
                kind: &str,
                namespace: Option<&str>,
                name: &str,
                content: String,
            ) -> Result<(), ConfWriterError> {
                self.$field.set_one(kind, namespace, name, content).await
            }

            async fn create_one(
                &self,
                kind: &str,
                namespace: Option<&str>,
                name: &str,
                content: String,
            ) -> Result<(), ConfWriterError> {
                self.$field.create_one(kind, namespace, name, content).await
            }

            async fn update_one(
                &self,
                kind: &str,
                namespace: Option<&str>,
                name: &str,
                content: String,
            ) -> Result<(), ConfWriterError> {
                self.$field.update_one(kind, namespace, name, content).await
            }

            async fn get_one(
                &self,
                kind: &str,
                namespace: Option<&str>,
                name: &str,
            ) -> Result<String, ConfWriterError> {
                self.$field.get_one(kind, namespace, name).await
            }

            async fn get_list_by_kind(
                &self,
                kind: &str,
                opts: Option<ListOptions>,
            ) -> Result<ListResult, ConfWriterError> {
                self.$field.get_list_by_kind(kind, opts).await
            }

            async fn get_list_by_kind_ns(
                &self,
                kind: &str,
                namespace: &str,
                opts: Option<ListOptions>,
            ) -> Result<ListResult, ConfWriterError> {
                self.$field.get_list_by_kind_ns(kind, namespace, opts).await
            }

            async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
                self.$field.delete_one(kind, namespace, name).await
            }

            async fn list_all(&self, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
                self.$field.list_all(opts).await
            }

            async fn list_keys_by_kind(
                &self,
                kind: &str,
                opts: Option<ListOptions>,
            ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
                self.$field.list_keys_by_kind(kind, opts).await
            }

            async fn list_keys_by_kind_ns(
                &self,
                kind: &str,
                namespace: &str,
                opts: Option<ListOptions>,
            ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
                self.$field.list_keys_by_kind_ns(kind, namespace, opts).await
            }
        }
    };
}
pub(crate) use impl_center_api_delegate;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_key_meta_is_constructible() {
        let _meta = ResourceKeyMeta {
            namespace: Some("ns".to_string()),
            name: "n".to_string(),
            creation_timestamp: None,
            labels: Default::default(),
            annotations: Default::default(),
            resource_version: Some("1".to_string()),
        };
        let _key = ResourceKey {
            kind: "HTTPRoute".to_string(),
            api_version: "gateway.networking.k8s.io/v1".to_string(),
            metadata: _meta,
        };
    }

    #[test]
    fn list_result_default_param_is_conf_entry() {
        let r: ListResult = ListResult {
            items: Vec::<ConfEntry>::new(),
            continue_token: None,
        };
        assert!(r.items.is_empty());
    }

    #[test]
    fn list_result_can_hold_resource_keys() {
        let r: ListResult<ResourceKey> = ListResult {
            items: Vec::new(),
            continue_token: Some("tok".to_string()),
        };
        assert_eq!(r.continue_token.as_deref(), Some("tok"));
    }

    #[test]
    fn not_supported_displays() {
        let e = ConfWriterError::NotSupported("foo".to_string());
        assert_eq!(format!("{e}"), "Not supported: foo");
    }

    #[test]
    fn stale_pagination_displays() {
        let e = ConfWriterError::StalePagination("token expired".to_string());
        assert_eq!(format!("{e}"), "Stale pagination: token expired");
    }

    /// Locks in the JSON wire shape: ResourceKey must emit `apiVersion`
    /// (camelCase), matching the Dashboard TS interface and the K8s
    /// convention. Without `#[serde(rename_all = "camelCase")]` serde
    /// emits the raw `api_version`, which silently breaks consumers.
    #[test]
    fn resource_key_serializes_camel_case_fields() {
        let key = ResourceKey {
            kind: "HTTPRoute".to_string(),
            api_version: "gateway.networking.k8s.io/v1".to_string(),
            metadata: ResourceKeyMeta {
                name: "r".to_string(),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&key).expect("serialize");
        assert!(
            json.contains("\"apiVersion\":\"gateway.networking.k8s.io/v1\""),
            "expected camelCase apiVersion, got: {json}"
        );
        assert!(
            !json.contains("api_version"),
            "must not leak snake_case api_version, got: {json}"
        );
    }
}

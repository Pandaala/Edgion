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

use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;
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
    #[serde(default)]
    pub name: String,
    #[serde(rename = "creationTimestamp", skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<k8s_openapi::jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub annotations: std::collections::BTreeMap<String, String>,
    #[serde(rename = "resourceVersion", skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
}

/// Annotation key for the multi-KB blob the K8s apiserver attaches on
/// `kubectl apply -f`. Both the trait-default projection (FS / Etcd) and
/// the K8s `project_partial_meta_to_key` (kubernetes/storage.rs) strip it on
/// the way out so the metadata-only response stays small.
pub(super) const LAST_APPLIED_KEY: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// Project a fully-loaded `ConfEntry` into a metadata-only `ResourceKey`.
///
/// The helper parses ONLY the `apiVersion` + `metadata` block of the entry's
/// content (YAML preferred, JSON fallback), drops the
/// `kubectl.kubernetes.io/last-applied-configuration` annotation (multi-KB
/// blob attached on `kubectl apply -f`), and falls back to the `ConfEntry`'s
/// authoritative `name` / `namespace` when the parsed metadata is missing
/// them.
///
/// Returns `None` if the content is neither valid YAML nor valid JSON —
/// callers using `filter_map` will silently skip such entries instead of
/// failing the whole list.
pub(super) fn project_conf_entry_to_key(kind: &str, entry: &ConfEntry) -> Option<ResourceKey> {
    #[derive(Deserialize)]
    struct Partial {
        #[serde(rename = "apiVersion", default)]
        api_version: Option<String>,
        #[serde(default)]
        metadata: Option<ResourceKeyMeta>,
    }

    let partial: Partial = yaml_serde::from_str(&entry.content)
        .or_else(|_| serde_json::from_str(&entry.content))
        .ok()?;

    let mut meta = partial.metadata.unwrap_or_default();
    meta.annotations.remove(LAST_APPLIED_KEY);

    if meta.namespace.is_none() {
        meta.namespace = entry.namespace.clone();
    }
    if meta.name.is_empty() {
        meta.name = entry.name.clone();
    }

    Some(ResourceKey {
        kind: kind.to_string(),
        api_version: partial.api_version.unwrap_or_default(),
        metadata: meta,
    })
}

/// Project a `ListResult<ConfEntry>` into a `ListResult<ResourceKey>` by
/// applying [`project_conf_entry_to_key`] to each entry. Invalid entries are
/// silently dropped (via `filter_map`); `continue_token` passes through.
///
/// This is the shared body of the trait-default `list_keys_by_kind*` impls
/// AND the unit test that verifies the chain — keeping them on one code path
/// prevents the test from drifting away from the production logic.
pub(super) fn project_conf_entry_list(kind: &str, full: ListResult<ConfEntry>) -> ListResult<ResourceKey> {
    let items = full
        .items
        .iter()
        .filter_map(|e| project_conf_entry_to_key(kind, e))
        .collect();
    ListResult {
        items,
        continue_token: full.continue_token,
    }
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
    /// literal; the underlying error (yaml_serde, serde_json, …) is boxed.
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
    ) -> Result<(), ConfWriterError> {
        self.update_one_if_version(kind, namespace, name, content, None).await
    }

    /// Update an existing resource when its current resourceVersion matches.
    /// `None` makes the write unconditional and last-writer-wins — the
    /// documented contract for callers that send no precondition, not a legacy
    /// carve-out. See
    /// `skills/04-review/admin-api/intentional-design/admin-api-write-precondition-opt-in.md`.
    async fn update_one_if_version(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
        expected_resource_version: Option<&str>,
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

    /// Delete a single configuration without a concurrency precondition.
    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
        self.delete_one_if_version(kind, namespace, name, None).await
    }

    /// Delete a single configuration when its current resourceVersion matches.
    /// `None` makes the delete unconditional and last-writer-wins — the
    /// documented contract for callers that send no precondition, not a legacy
    /// carve-out. The admin `wipe` handler (`api/wipe_handlers.rs`) deletes
    /// through the unconditional `delete_one` wrapper by design. See
    /// `skills/04-review/admin-api/intentional-design/admin-api-write-precondition-opt-in.md`.
    async fn delete_one_if_version(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        expected_resource_version: Option<&str>,
    ) -> Result<(), ConfWriterError>;

    /// List all configurations across all kinds.
    ///
    /// # Arguments
    /// * `opts` - Pagination options. `None` returns all items, `Some(opts)` applies pagination.
    async fn list_all(&self, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError>;

    /// List keys (metadata-only) by kind, across all namespaces / cluster-scoped.
    ///
    /// Default implementation delegates to [`get_list_by_kind`] and projects
    /// the result via [`project_conf_entry_list`]. Backends with a native
    /// metadata-only path (e.g. K8s `Api::list_metadata`) override this for
    /// efficiency; FS / Etcd inherit the default.
    async fn list_keys_by_kind(
        &self,
        kind: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
        let full = self.get_list_by_kind(kind, opts).await?;
        Ok(project_conf_entry_list(kind, full))
    }

    /// List keys (metadata-only) by kind within a single namespace.
    ///
    /// Default implementation delegates to [`get_list_by_kind_ns`] and projects
    /// the result via [`project_conf_entry_list`].
    async fn list_keys_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
        let full = self.get_list_by_kind_ns(kind, namespace, opts).await?;
        Ok(project_conf_entry_list(kind, full))
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
    fn conf_dir_registry(&self) -> Option<Arc<crate::conf_mgr::conf_center::file_system::PathRegistry>> {
        None
    }

    /// Per-leadership-term conf_sync stream cancellation slot (spec §6.1).
    ///
    /// The etcd election installs a fresh token at each campaign win and cancels
    /// it at demotion, so a demoted leader's conf_sync streams are actively
    /// closed instead of silently hanging (the gateway keys its leader signal on
    /// the `watch_server_meta` stream ending). Default `None` = leadership terms
    /// not applicable (Kubernetes / FileSystem); the gRPC server then scopes
    /// streams to the process shutdown token exactly as before.
    fn term_token_slot(&self) -> Option<Arc<arc_swap::ArcSwap<tokio_util::sync::CancellationToken>>> {
        None
    }

    /// This center's view of controller leadership (spec §6.2).
    ///
    /// Default `NotApplicable` = leadership gating does not apply and the Admin
    /// API 409 gate is disabled (Kubernetes routes admin traffic to the leader
    /// via the leader Service; FileSystem is single-node). Only `EtcdCenter`
    /// overrides this (S3/S4 zero-K8s-impact guard).
    fn leadership_status(&self) -> LeadershipStatus {
        LeadershipStatus::NotApplicable
    }

    /// Live leadership signal for process-wide singleton components.
    ///
    /// Today the sole consumer is the federation client supervisor: the
    /// federation session is keyed by `controller_id` (`cluster/name`), which is
    /// identical on every replica, so exactly one replica may hold it. The
    /// receiver carries `true` while this process is the leader.
    ///
    /// This is ORTHOGONAL to [`CenterLifeCycle::leadership_status`], and the two
    /// deliberately DISAGREE in Kubernetes mode: `leadership_status` feeds the
    /// Admin API not-leader 409 gate and must stay `NotApplicable` there
    /// (Kubernetes routes admin traffic to the leader via the `edgion.io/leader`
    /// pod-label Service selector, and in `ha_mode: all-serve` followers must
    /// keep serving reads), yet `leader_signal` DOES return `Some` there. Do not
    /// "unify" them.
    ///
    /// `None` means this backend has no leader election at all (FileSystem), so
    /// the caller is always eligible and must behave exactly as if the signal
    /// were permanently `true`.
    fn leader_signal(&self) -> Option<tokio::sync::watch::Receiver<bool>> {
        None
    }
}

/// A center's leadership view, consumed by the Admin API not-leader gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeadershipStatus {
    /// This center has no leader election (Kubernetes / FileSystem) — the gate
    /// is disabled.
    NotApplicable,
    /// This process is the current leader — serve normally.
    Leader,
    /// This process is a standby. `leader_admin_addr` names the current leader's
    /// admin endpoint when known (`None` during churn / before first refresh).
    Follower { leader_admin_addr: Option<String> },
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

            async fn update_one_if_version(
                &self,
                kind: &str,
                namespace: Option<&str>,
                name: &str,
                content: String,
                expected_resource_version: Option<&str>,
            ) -> Result<(), ConfWriterError> {
                self.$field
                    .update_one_if_version(kind, namespace, name, content, expected_resource_version)
                    .await
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

            async fn delete_one_if_version(
                &self,
                kind: &str,
                namespace: Option<&str>,
                name: &str,
                expected_resource_version: Option<&str>,
            ) -> Result<(), ConfWriterError> {
                self.$field
                    .delete_one_if_version(kind, namespace, name, expected_resource_version)
                    .await
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
#[path = "../../../tests/unit/conf_mgr/conf_center/traits_test.rs"]
mod tests;

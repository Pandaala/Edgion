//! ConfMgr - Unified configuration manager facade
//!
//! This is the main entry point for configuration management.
//! It holds an `Arc<dyn ConfCenter>` and delegates all operations to it.
//!
//! ## Architecture
//!
//! ```text
//! ConfMgr (facade)
//! └── Arc<dyn ConfCenter>
//!     ├── FileSystemCenter (FileSystem mode)
//!     └── KubernetesCenter (Kubernetes mode)
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! // Create ConfMgr based on configuration
//! let conf_mgr = Arc::new(ConfMgr::create(config).await?);
//!
//! // Start the configuration center
//! conf_mgr.start_with_shutdown(shutdown_handle).await?;
//!
//! // Access CRUD operations
//! let content = conf_mgr.get_one("HTTPRoute", Some("default"), "my-route").await?;
//! ```

use super::conf_center::ConfCenterConfig;
use super::conf_center::file_system::FileSystemCenter;
use super::conf_center::kubernetes::KubernetesCenter;
use super::conf_center::traits::{CenterApi, CenterLifeCycle, ConfCenter, ConfWriterError, ListOptions, ListResult};
use super::sync_runtime::ShutdownHandle;
use crate::core::conf_sync::conf_server::ConfigSyncServer;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// ConfMgr - Unified configuration manager
///
/// This is a facade that holds an `Arc<dyn ConfCenter>` and delegates all operations.
/// The actual implementation is provided by `FileSystemCenter` or `KubernetesCenter`.
pub struct ConfMgr {
    conf_center: Arc<dyn ConfCenter>,
}

impl ConfMgr {
    /// Create a new ConfMgr based on configuration
    ///
    /// This factory method creates the appropriate implementation based on the configuration:
    /// - FileSystem mode: Creates `FileSystemCenter`
    /// - Kubernetes mode: Creates `KubernetesCenter`
    pub async fn create(config: ConfCenterConfig) -> Result<Self> {
        let conf_center: Arc<dyn ConfCenter> = match config {
            ConfCenterConfig::FileSystem(fs_config) => {
                tracing::info!(
                    component = "conf_mgr",
                    mode = "file_system",
                    conf_dir = %fs_config.conf_dir().display(),
                    "Creating FileSystemCenter"
                );
                Arc::new(FileSystemCenter::new(fs_config)?)
            }
            ConfCenterConfig::Kubernetes(k8s_config) => {
                tracing::info!(
                    component = "conf_mgr",
                    mode = "kubernetes",
                    gateway_class = %k8s_config.gateway_class(),
                    "Creating KubernetesCenter"
                );
                Arc::new(KubernetesCenter::new(k8s_config).await?)
            }
        };

        Ok(Self { conf_center })
    }

    /// Get the underlying ConfCenter
    ///
    /// This allows direct access to the `Arc<dyn ConfCenter>` for advanced use cases.
    pub fn conf_center(&self) -> Arc<dyn ConfCenter> {
        self.conf_center.clone()
    }

    // ==================== Lifecycle Delegation ====================

    /// Start the configuration center with an external shutdown handle
    ///
    /// This is the preferred method when the caller manages signal handling.
    /// The shutdown handle will be used to coordinate graceful shutdown.
    pub async fn start_with_shutdown(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        self.conf_center.start(shutdown_handle).await
    }

    /// Start the configuration center (creates its own shutdown handler)
    ///
    /// This method creates its own ShutdownHandle and signal listener.
    /// Use `start_with_shutdown()` if the caller manages signal handling.
    pub async fn start(&self) -> Result<()> {
        // Create shutdown handle and start signal listener
        let shutdown_handle = ShutdownHandle::new();
        let signal_shutdown = shutdown_handle.clone();
        tokio::spawn(async move {
            signal_shutdown.wait_for_signals().await;
        });

        self.start_with_shutdown(shutdown_handle).await
    }

    /// Reload all resources (FileSystem mode only)
    pub async fn reload(&self) -> Result<()> {
        self.conf_center.reload().await
    }

    /// Check if the system is ready
    pub fn is_ready(&self) -> bool {
        self.conf_center.is_ready()
    }

    /// Get the ConfigSyncServer (may be None if not ready)
    pub fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.conf_center.config_sync_server()
    }

    /// Check if running in Kubernetes mode
    pub fn is_k8s_mode(&self) -> bool {
        self.conf_center.is_k8s_mode()
    }
}

// ============================================================================
// CenterApi implementation - delegates to conf_center
// ============================================================================

#[async_trait]
impl CenterApi for ConfMgr {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.conf_center.set_one(kind, namespace, name, content).await
    }

    async fn create_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.conf_center.create_one(kind, namespace, name, content).await
    }

    async fn update_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        self.conf_center.update_one(kind, namespace, name, content).await
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError> {
        self.conf_center.get_one(kind, namespace, name).await
    }

    async fn get_list_by_kind(&self, kind: &str, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        self.conf_center.get_list_by_kind(kind, opts).await
    }

    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult, ConfWriterError> {
        self.conf_center.get_list_by_kind_ns(kind, namespace, opts).await
    }

    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfWriterError> {
        self.conf_center.cnt_by_kind(kind).await
    }

    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfWriterError> {
        self.conf_center.cnt_by_kind_ns(kind, namespace).await
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
        self.conf_center.delete_one(kind, namespace, name).await
    }

    async fn list_all(&self, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        self.conf_center.list_all(opts).await
    }
}

// ============================================================================
// CenterLifeCycle implementation - delegates to conf_center
// ============================================================================

#[async_trait]
impl CenterLifeCycle for ConfMgr {
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        self.conf_center.start(shutdown_handle).await
    }

    async fn reload(&self) -> Result<()> {
        self.conf_center.reload().await
    }

    fn is_ready(&self) -> bool {
        self.conf_center.is_ready()
    }

    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.conf_center.config_sync_server()
    }

    fn is_k8s_mode(&self) -> bool {
        self.conf_center.is_k8s_mode()
    }
}

// ConfMgr automatically implements ConfCenter via blanket impl
// because it implements both CenterApi and CenterLifeCycle

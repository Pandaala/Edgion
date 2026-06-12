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

use super::conf_center::etcd::EtcdCenter;
use super::conf_center::file_system::FileSystemCenter;
use super::conf_center::kubernetes::KubernetesCenter;
use super::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfCenter, ConfWriterError, ListOptions, ListResult,
    ResourceKey,
};
use super::conf_center::ConfCenterConfig;
use super::sync_runtime::ShutdownHandle;
use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::CrossNsRevalidationListener;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;
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
            ConfCenterConfig::Etcd(etcd_config) => {
                tracing::info!(
                    component = "conf_mgr",
                    mode = "etcd",
                    endpoints = ?etcd_config.endpoints,
                    key_prefix = %etcd_config.key_prefix,
                    "Creating EtcdCenter"
                );
                Arc::new(EtcdCenter::new(etcd_config).await?)
            }
        };

        // Register cross-namespace revalidation listener
        // This ensures that when ReferenceGrant changes, affected routes are requeued
        let runtime = conf_center.runtime();
        let listener = Arc::new(CrossNsRevalidationListener::new(runtime.clone()));
        runtime.ref_grant_dispatcher.register_listener(listener);
        tracing::debug!(component = "conf_mgr", "Registered CrossNsRevalidationListener");

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

    /// Collect a stats snapshot for fed_sync push if anything has changed since
    /// last call. Returns `Some((per_kind, total))` when at least one processor
    /// is dirty (and clears the dirty flags); otherwise `None`.
    pub fn collect_stats_if_dirty(&self) -> Option<(std::collections::HashMap<String, u32>, u32)> {
        self.conf_center.runtime().processor_registry.collect_stats_if_dirty()
    }

    /// Request a reload (re-initialize all processors and stores)
    ///
    /// This triggers a full restart of the configuration center:
    /// 1. Stop current controllers
    /// 2. Clear PROCESSOR_REGISTRY
    /// 3. Create new ConfigSyncServer (new server_id)
    /// 4. Restart controllers (full Init -> InitApply -> InitDone flow)
    ///
    /// Returns Ok(()) if the reload request was accepted (reload happens asynchronously).
    pub fn request_reload(&self) -> Result<(), String> {
        self.conf_center.request_reload()
    }
}

// CenterApi implementation - delegates to `conf_center` via macro
impl_center_api_delegate!(ConfMgr, conf_center);

// ============================================================================
// CenterLifeCycle implementation - delegates to conf_center
// ============================================================================

#[async_trait]
impl CenterLifeCycle for ConfMgr {
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        self.conf_center.start(shutdown_handle).await
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

    fn request_reload(&self) -> Result<(), String> {
        self.conf_center.request_reload()
    }

    fn runtime(&self) -> Arc<ControllerRuntime> {
        self.conf_center.runtime()
    }
}

// ConfMgr automatically implements ConfCenter via blanket impl
// because it implements both CenterApi and CenterLifeCycle
